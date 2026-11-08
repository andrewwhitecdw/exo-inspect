//! Minimal reader for the classic netCDF file formats (CDF-1 and CDF-2),
//! sufficient to inspect ExodusII files produced by MOOSE/libMesh.
//!
//! Everything is big-endian. The implementation covers the format as
//! documented in the netCDF classic format specification: dimensions,
//! global attributes, variables (with per-variable attributes), record
//! variables, and strided reads. HDF5-based netCDF-4 files and CDF-5 are
//! rejected with a clear error.

use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};

/// Errors produced while opening or reading a netCDF file.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The file does not start with the netCDF magic bytes.
    #[error("not a netCDF file")]
    NotNetCdf,
    /// The classic-format version byte is not 1 or 2 (HDF5 netCDF-4 shows
    /// up here as the HDF magic and is rejected earlier).
    #[error("unsupported netCDF classic version {0}")]
    UnsupportedVersion(u8),
    /// The header ended before the documented fields were complete.
    #[error("truncated header while reading {0}")]
    Truncated(&'static str),
    /// A list tag in the header was not the expected constant.
    #[error("corrupt header: bad list tag {0:#x}")]
    BadTag(u32),
    /// A dimension referenced by a variable does not exist.
    #[error("corrupt header: bad dimension id {0}")]
    BadDim(usize),
    /// A required dimension is missing from the file.
    #[error("missing dimension '{0}'")]
    MissingDim(String),
    /// A required variable is missing from the file.
    #[error("missing variable '{0}'")]
    MissingVar(String),
    /// An attribute that was expected to hold text did not.
    #[error("attribute '{0}' is not text")]
    AttrNotText(String),
    /// Underlying I/O failure.
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// netCDF primitive types used by classic files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NcType {
    /// 1-byte signed integer.
    Byte,
    /// 1-byte character.
    Char,
    /// 2-byte signed integer.
    Short,
    /// 4-byte signed integer.
    Int,
    /// 4-byte float.
    Float,
    /// 8-byte float.
    Double,
}

impl NcType {
    fn from_tag(tag: u32) -> Option<Self> {
        match tag {
            1 => Some(NcType::Byte),
            2 => Some(NcType::Char),
            3 => Some(NcType::Short),
            4 => Some(NcType::Int),
            5 => Some(NcType::Float),
            6 => Some(NcType::Double),
            _ => None,
        }
    }

    /// Size of one value in bytes.
    pub fn size(self) -> usize {
        match self {
            NcType::Byte | NcType::Char => 1,
            NcType::Short => 2,
            NcType::Int | NcType::Float => 4,
            NcType::Double => 8,
        }
    }
}

/// A dimension from the file header.
#[derive(Debug, Clone)]
pub struct Dim {
    /// Dimension name (e.g. `num_nodes`, `time_step`).
    pub name: String,
    /// Length; `None` for the unlimited (record) dimension.
    pub len: Option<usize>,
}

/// A variable attribute (also used for global attributes).
#[derive(Debug, Clone)]
pub struct Attr {
    /// Attribute name.
    pub name: String,
    /// Raw attribute values.
    pub value: AttrValue,
}

impl Attr {
    /// Attribute text with trailing NULs and padding stripped.
    pub fn text(&self) -> Result<&str, Error> {
        match &self.value {
            AttrValue::Chars(s) => Ok(s.trim_end_matches(['\0', ' '])),
            _ => Err(Error::AttrNotText(self.name.clone())),
        }
    }
}

/// Typed attribute payload.
#[derive(Debug, Clone)]
pub enum AttrValue {
    /// Character data.
    Chars(String),
    /// Any numeric payload kept as raw little-endian-free raw bytes.
    Raw(Vec<u8>, NcType),
}

/// A variable from the file header.
#[derive(Debug, Clone)]
pub struct Var {
    /// Variable name.
    pub name: String,
    /// Dimension indices, in C order (last dimension varies fastest).
    pub dimids: Vec<usize>,
    /// Per-variable attributes (ExodusII stores e.g. `elem_type` here).
    pub atts: Vec<Attr>,
    /// Primitive type.
    pub nctype: NcType,
    /// Padded size of one record slab in bytes.
    pub vsize: usize,
    /// Offset of the variable's data from the start of the file.
    pub begin: u64,
}

impl Var {
    /// Look up a per-variable attribute by name.
    pub fn attr(&self, name: &str) -> Option<&Attr> {
        self.atts.iter().find(|a| a.name == name)
    }
}

/// A parsed classic netCDF header.
#[derive(Debug)]
pub struct FileMeta {
    /// `true` for CDF-2 (64-bit offsets).
    pub is_64bit: bool,
    /// Number of records (length of the unlimited dimension).
    pub numrecs: usize,
    dims: Vec<Dim>,
    vars: Vec<Var>,
    /// Sum of the padded sizes of all record variables.
    recsize: usize,
}

impl FileMeta {
    /// All dimensions.
    pub fn dims(&self) -> &[Dim] {
        &self.dims
    }

    /// All variables.
    pub fn vars(&self) -> &[Var] {
        &self.vars
    }

    /// Find a dimension by name.
    pub fn dim(&self, name: &str) -> Option<&Dim> {
        self.dims.iter().find(|d| d.name == name)
    }

    /// Length of a named dimension; the unlimited dimension reports the
    /// record count.
    pub fn dim_len(&self, name: &str) -> Result<usize, Error> {
        let d = self
            .dim(name)
            .ok_or_else(|| Error::MissingDim(name.to_string()))?;
        Ok(d.len.unwrap_or(self.numrecs))
    }

    /// Find a variable by name.
    pub fn var(&self, name: &str) -> Option<&Var> {
        self.vars.iter().find(|v| v.name == name)
    }

    /// Require a variable by name.
    pub fn require_var(&self, name: &str) -> Result<&Var, Error> {
        self.var(name)
            .ok_or_else(|| Error::MissingVar(name.to_string()))
    }
}

/// Padding to the next 4-byte boundary.
fn pad4(n: usize) -> usize {
    n.div_ceil(4) * 4
}

struct HeaderReader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> HeaderReader<'a> {
    fn u32(&mut self, what: &'static str) -> Result<u32, Error> {
        let end = self.pos.checked_add(4).ok_or(Error::Truncated(what))?;
        let bytes: [u8; 4] = self
            .buf
            .get(self.pos..end)
            .and_then(|s| s.try_into().ok())
            .ok_or(Error::Truncated(what))?;
        self.pos = end;
        Ok(u32::from_be_bytes(bytes))
    }

    fn u64(&mut self, what: &'static str) -> Result<u64, Error> {
        let end = self.pos.checked_add(8).ok_or(Error::Truncated(what))?;
        let bytes: [u8; 8] = self
            .buf
            .get(self.pos..end)
            .and_then(|s| s.try_into().ok())
            .ok_or(Error::Truncated(what))?;
        self.pos = end;
        Ok(u64::from_be_bytes(bytes))
    }

    fn name(&mut self, what: &'static str) -> Result<String, Error> {
        let len = self.u32(what)? as usize;
        let start = self.pos;
        let end = start.checked_add(len).ok_or(Error::Truncated(what))?;
        let raw = self.buf.get(start..end).ok_or(Error::Truncated(what))?;
        let s = String::from_utf8_lossy(raw).into_owned();
        self.pos = start + pad4(len);
        Ok(s)
    }

    fn values(
        &mut self,
        nctype: NcType,
        nelems: usize,
        what: &'static str,
    ) -> Result<AttrValue, Error> {
        let raw_len = nctype
            .size()
            .checked_mul(nelems)
            .ok_or(Error::Truncated(what))?;
        let end = self
            .pos
            .checked_add(raw_len)
            .ok_or(Error::Truncated(what))?;
        let raw = self.buf.get(self.pos..end).ok_or(Error::Truncated(what))?;
        self.pos += pad4(raw_len);
        match nctype {
            NcType::Char => Ok(AttrValue::Chars(String::from_utf8_lossy(raw).into_owned())),
            _ => Ok(AttrValue::Raw(raw.to_vec(), nctype)),
        }
    }

    fn attr_list(&mut self, what: &'static str) -> Result<Vec<Attr>, Error> {
        let tag = self.u32(what)?;
        if tag == 0 {
            let zero = self.u32(what)?;
            if zero != 0 {
                return Err(Error::BadTag(zero));
            }
            return Ok(Vec::new());
        }
        if tag != 0x0C {
            return Err(Error::BadTag(tag));
        }
        let nelems = self.u32(what)? as usize;
        let mut out = Vec::new();
        for _ in 0..nelems {
            let name = self.name(what)?;
            let nctype = NcType::from_tag(self.u32(what)?).ok_or(Error::BadTag(0))?;
            let n = self.u32(what)? as usize;
            let value = self.values(nctype, n, what)?;
            out.push(Attr { name, value });
        }
        Ok(out)
    }
}

/// Parse the header from the start of `buf` (the first few KiB of a file).
pub fn parse_header(buf: &[u8]) -> Result<FileMeta, Error> {
    if buf.len() < 4 || &buf[0..3] != b"CDF" {
        return Err(Error::NotNetCdf);
    }
    let version = buf[3];
    if version != 1 && version != 2 {
        return Err(Error::UnsupportedVersion(version));
    }
    let mut r = HeaderReader { buf, pos: 4 };
    let numrecs = r.u32("numrecs")? as usize;

    // Dimension list.
    let tag = r.u32("dimension list")?;
    let mut dims = Vec::new();
    if tag == 0 {
        let zero = r.u32("dimension list")?;
        if zero != 0 {
            return Err(Error::BadTag(zero));
        }
    } else {
        if tag != 0x0A {
            return Err(Error::BadTag(tag));
        }
        let ndims = r.u32("dimension list")? as usize;
        for _ in 0..ndims {
            let name = r.name("dimension name")?;
            let len = r.u32("dimension size")? as usize;
            // A size of 0 marks the single unlimited (record) dimension;
            // libmesh/exodus do not guarantee it comes first.
            let dim_len = if len == 0 { None } else { Some(len) };
            dims.push(Dim { name, len: dim_len });
        }
    }

    // Global attributes.
    r.attr_list("global attributes")?;

    // Variable list.
    let tag = r.u32("variable list")?;
    let mut vars = Vec::new();
    if tag == 0 {
        let zero = r.u32("variable list")?;
        if zero != 0 {
            return Err(Error::BadTag(zero));
        }
    } else {
        if tag != 0x0B {
            return Err(Error::BadTag(tag));
        }
        let nvars = r.u32("variable list")? as usize;
        for _ in 0..nvars {
            let name = r.name("variable name")?;
            let ndims = r.u32("variable rank")? as usize;
            let mut dimids = Vec::with_capacity(ndims);
            for _ in 0..ndims {
                let id = r.u32("variable dimension")? as usize;
                if id >= dims.len() {
                    return Err(Error::BadDim(id));
                }
                dimids.push(id);
            }
            let atts = r.attr_list("variable attributes")?;
            let nctype = NcType::from_tag(r.u32("variable type")?).ok_or(Error::BadTag(0))?;
            let vsize = r.u32("variable size")? as usize;
            let begin = if version == 2 {
                r.u64("variable offset")?
            } else {
                r.u32("variable offset")? as u64
            };
            vars.push(Var {
                name,
                dimids,
                atts,
                nctype,
                vsize,
                begin,
            });
        }
    }

    let recsize = vars
        .iter()
        .filter(|v| v.dimids.first().is_some_and(|&d| dims[d].len.is_none()))
        .map(|v| v.vsize)
        .sum();

    Ok(FileMeta {
        is_64bit: version == 2,
        numrecs,
        dims,
        vars,
        recsize,
    })
}

/// An open classic netCDF file with positioned reads.
pub struct Reader<R> {
    src: R,
    /// Parsed header.
    pub meta: FileMeta,
}

impl Reader<BufReader<File>> {
    /// Open a file and parse its header.
    pub fn open(path: &std::path::Path) -> Result<Self, Error> {
        let mut file = File::open(path)?;
        let mut header = vec![0u8; 64 * 1024];
        let n = file.read(&mut header)?;
        header.truncate(n);
        let meta = parse_header(&header)?;
        Ok(Reader {
            src: BufReader::new(file),
            meta,
        })
    }
}

impl<R: Read + Seek> Reader<R> {
    /// Read `count` values of `ty` at an absolute file offset.
    fn read_at(&mut self, offset: u64, ty: NcType, count: usize) -> Result<Vec<f64>, Error> {
        self.src.seek(SeekFrom::Start(offset))?;
        let nbytes = ty
            .size()
            .checked_mul(count)
            .ok_or(Error::Truncated("variable data"))?;
        let mut raw = vec![0u8; nbytes];
        self.src.read_exact(&mut raw)?;
        let mut out = Vec::with_capacity(count);
        for chunk in raw.chunks_exact(ty.size()) {
            out.push(match ty {
                NcType::Byte => chunk[0] as i8 as f64,
                NcType::Char => chunk[0] as f64,
                NcType::Short => i16::from_be_bytes([chunk[0], chunk[1]]) as f64,
                NcType::Int => i32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]) as f64,
                NcType::Float => {
                    f32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]) as f64
                }
                NcType::Double => f64::from_be_bytes([
                    chunk[0], chunk[1], chunk[2], chunk[3], chunk[4], chunk[5], chunk[6], chunk[7],
                ]),
            });
        }
        Ok(out)
    }

    /// Offset of one element of a (possibly record) variable.
    ///
    /// `coords` has one entry per dimension of the variable; the unlimited
    /// dimension (which must be the first) contributes through the record
    /// stride. Strides follow C order: the last dimension varies fastest.
    fn element_offset(&self, var: &Var, coords: &[usize]) -> Result<u64, Error> {
        if coords.len() != var.dimids.len() {
            return Err(Error::BadDim(coords.len()));
        }
        let mut offset = var.begin;
        let mut stride: usize = 1;
        for (i, (&c, &d)) in coords.iter().zip(&var.dimids).enumerate().rev() {
            if i == 0 && self.meta.dims[d].len.is_none() {
                offset += c as u64 * self.meta.recsize as u64;
            } else {
                offset += c as u64 * stride as u64 * var.nctype.size() as u64;
            }
            stride = stride
                .checked_mul(self.meta.dims[d].len.unwrap_or(1))
                .ok_or(Error::BadDim(d))?;
        }
        Ok(offset)
    }

    /// Read one variable element as f64.
    pub fn value(&mut self, var: &Var, coords: &[usize]) -> Result<f64, Error> {
        let offset = self.element_offset(var, coords)?;
        Ok(self.read_at(offset, var.nctype, 1)?[0])
    }

    /// Read a whole non-record variable as f64 values.
    pub fn var_values(&mut self, var: &Var) -> Result<Vec<f64>, Error> {
        let count: usize = var
            .dimids
            .iter()
            .map(|&d| self.meta.dims[d].len.unwrap_or(1))
            .product();
        self.read_at(var.begin, var.nctype, count)
    }

    /// Iterate over the records of a record variable, yielding the f64
    /// values of one record slab at a time.
    pub fn for_each_record(
        &mut self,
        var: &Var,
        mut f: impl FnMut(usize, &[f64]),
    ) -> Result<(), Error> {
        let unlimited_first = var
            .dimids
            .first()
            .is_some_and(|&d| self.meta.dims[d].len.is_none());
        if !unlimited_first {
            return Err(Error::BadDim(var.dimids[0]));
        }
        let per_record: usize = var
            .dimids
            .iter()
            .skip(1)
            .map(|&d| self.meta.dims[d].len.unwrap_or(1))
            .product();
        for rec in 0..self.meta.numrecs {
            let offset = var.begin + rec as u64 * self.meta.recsize as u64;
            let values = self.read_at(offset, var.nctype, per_record)?;
            f(rec, &values);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a tiny CDF-1 file: unlimited `time_step` dim (2 records),
    /// `num_nodes` = 4, `time_whole` record var, `coordx` var, one global
    /// attribute, and an `elem_type` attribute on `coordx`.
    fn fixture() -> Vec<u8> {
        let mut b: Vec<u8> = Vec::new();
        b.extend_from_slice(b"CDF\x01");
        b.extend_from_slice(&2u32.to_be_bytes()); // numrecs
        // dims
        b.extend_from_slice(&0x0Au32.to_be_bytes());
        b.extend_from_slice(&2u32.to_be_bytes());
        for (name, size) in [("time_step", 0u32), ("num_nodes", 4u32)] {
            b.extend_from_slice(&(name.len() as u32).to_be_bytes());
            b.extend_from_slice(name.as_bytes());
            b.resize(b.len() + (4 - name.len() % 4) % 4, 0);
            b.extend_from_slice(&size.to_be_bytes());
        }
        // global atts
        b.extend_from_slice(&0u32.to_be_bytes());
        b.extend_from_slice(&0u32.to_be_bytes());
        // vars
        b.extend_from_slice(&0x0Bu32.to_be_bytes());
        b.extend_from_slice(&2u32.to_be_bytes());

        let var_header = |name: &str, dimids: &[u32], with_att: bool| -> Vec<u8> {
            let mut v = Vec::new();
            v.extend_from_slice(&(name.len() as u32).to_be_bytes());
            v.extend_from_slice(name.as_bytes());
            v.resize(v.len() + (4 - name.len() % 4) % 4, 0);
            v.extend_from_slice(&(dimids.len() as u32).to_be_bytes());
            for d in dimids {
                v.extend_from_slice(&d.to_be_bytes());
            }
            if with_att {
                v.extend_from_slice(&0x0Cu32.to_be_bytes());
                v.extend_from_slice(&1u32.to_be_bytes());
                let att = "elem_type";
                v.extend_from_slice(&(att.len() as u32).to_be_bytes());
                v.extend_from_slice(att.as_bytes());
                v.resize(v.len() + (4 - att.len() % 4) % 4, 0);
                v.extend_from_slice(&2u32.to_be_bytes()); // char
                v.extend_from_slice(&5u32.to_be_bytes()); // QUAD4
                v.extend_from_slice(b"QUAD4");
                v.resize(v.len() + 3, 0); // pad the 5-byte value to 8
            } else {
                v.extend_from_slice(&0u32.to_be_bytes());
                v.extend_from_slice(&0u32.to_be_bytes());
            }
            v
        };

        b.extend(var_header("time_whole", &[0], false));
        b.extend_from_slice(&6u32.to_be_bytes()); // double
        b.extend_from_slice(&8u32.to_be_bytes()); // vsize: 1 double
        let begin1 = b.len();
        b.extend_from_slice(&0u32.to_be_bytes()); // begin placeholder

        b.extend(var_header("coordx", &[1], true));
        b.extend_from_slice(&6u32.to_be_bytes());
        b.extend_from_slice(&32u32.to_be_bytes()); // vsize: 4 doubles
        let begin2 = b.len();
        b.extend_from_slice(&0u32.to_be_bytes()); // begin placeholder

        // Data follows the whole header: two records of time_whole
        // (recsize 8), then coordx (4 doubles).
        let data_start = b.len() as u32;
        b[begin1..begin1 + 4].copy_from_slice(&data_start.to_be_bytes());
        b[begin2..begin2 + 4].copy_from_slice(&(data_start + 16).to_be_bytes());

        // data: 2 records of time_whole (recsize 8), then coordx (4 doubles)
        for t in [1.5f64, 2.5f64] {
            b.extend_from_slice(&t.to_be_bytes());
        }
        for x in [0.0f64, 0.25, 0.5, 0.75] {
            b.extend_from_slice(&x.to_be_bytes());
        }
        b
    }

    #[test]
    fn parses_header_and_reads_values() {
        let bytes = fixture();
        let meta = parse_header(&bytes).expect("header parses");
        assert_eq!(meta.numrecs, 2);
        assert_eq!(meta.dims.len(), 2);
        assert_eq!(meta.dim_len("num_nodes").expect("num_nodes"), 4);
        assert_eq!(meta.dim_len("time_step").expect("time_step"), 2);
        let tw = meta.require_var("time_whole").expect("time_whole");
        assert_eq!(tw.nctype, NcType::Double);
        let coordx = meta.require_var("coordx").expect("coordx");
        let att = coordx.attr("elem_type").expect("elem_type attr");
        assert_eq!(att.text().expect("text"), "QUAD4");

        let mut r = Reader {
            src: std::io::Cursor::new(bytes),
            meta,
        };
        let tw = r
            .meta
            .require_var("time_whole")
            .expect("time_whole")
            .clone();
        assert_eq!(r.value(&tw, &[1]).expect("t1"), 2.5);

        let mut times = Vec::new();
        r.for_each_record(&tw, |_, vals| times.push(vals[0]))
            .expect("records");
        assert_eq!(times, [1.5, 2.5]);

        let coordx = r.meta.require_var("coordx").expect("coordx").clone();
        let xs = r.var_values(&coordx).expect("coordx values");
        assert_eq!(xs, [0.0, 0.25, 0.5, 0.75]);
    }

    #[test]
    fn rejects_hdf5_magic() {
        let buf = b"\x89HDF\r\n\x1a\n";
        assert!(matches!(parse_header(buf), Err(Error::NotNetCdf)));
    }

    #[test]
    fn rejects_cdf5() {
        let mut buf = b"CDF\x05".to_vec();
        buf.extend_from_slice(&[0; 32]);
        assert!(matches!(
            parse_header(&buf),
            Err(Error::UnsupportedVersion(5))
        ));
    }
}
