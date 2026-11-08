//! ExodusII-specific interpretation on top of the classic netCDF reader.

use crate::netcdf::{Error, Reader};

/// One element block.
#[derive(Debug, Clone)]
pub struct Block {
    /// 1-based block index.
    pub index: usize,
    /// Block ID (`eb_prop1` value).
    pub id: i64,
    /// Number of elements in the block.
    pub num_elems: usize,
    /// Nodes per element (0 when not recorded).
    pub nodes_per_elem: usize,
    /// Element type from the `elem_type` attribute (e.g. `QUAD4`).
    pub elem_type: Option<String>,
}

/// One node set or side set.
#[derive(Debug, Clone)]
pub struct Set {
    /// 1-based set index.
    pub index: usize,
    /// Set ID (`ns_prop1`/`ss_prop1` value).
    pub id: i64,
    /// Entry count: nodes for node sets, sides for side sets.
    pub count: usize,
}

/// A summarized view of an ExodusII file.
#[derive(Debug, Clone)]
pub struct Summary {
    /// Contents of the first QA record (code, version, date, time), when present.
    pub qa: Vec<String>,
    /// Spatial dimension (2 or 3).
    pub num_dim: usize,
    /// Total number of nodes.
    pub num_nodes: usize,
    /// Total number of elements.
    pub num_elems: usize,
    /// Element blocks.
    pub blocks: Vec<Block>,
    /// Node sets.
    pub node_sets: Vec<Set>,
    /// Side sets.
    pub side_sets: Vec<Set>,
    /// Time step values from `time_whole`.
    pub times: Option<Vec<f64>>,
    /// Nodal variable names.
    pub nodal_vars: Vec<String>,
    /// Elemental variable names.
    pub elem_vars: Vec<String>,
    /// Global variable names.
    pub global_vars: Vec<String>,
    /// Node-set variable names.
    pub ns_vars: Vec<String>,
    /// Side-set variable names.
    pub ss_vars: Vec<String>,
}

/// Read the summary from an open file.
pub fn summarize(
    reader: &mut Reader<impl std::io::Read + std::io::Seek>,
) -> Result<Summary, Error> {
    let meta = &reader.meta;
    let num_dim = meta.dim_len("num_dim").unwrap_or(0);
    let num_nodes = meta.dim_len("num_nodes").unwrap_or(0);
    let num_elems = meta.dim_len("num_elem").unwrap_or(0);
    let num_blk = meta.dim_len("num_el_blk").unwrap_or(0);
    let num_ns = meta.dim_len("num_node_sets").unwrap_or(0);
    let num_ss = meta.dim_len("num_side_sets").unwrap_or(0);

    let qa = read_qa(reader).unwrap_or_default();
    let times = read_times(reader);
    let nodal_vars = read_names(reader, "name_nod_var", "num_nod_var");
    let elem_vars = read_names(reader, "name_elem_var", "num_elem_var");
    let global_vars = read_names(reader, "name_glo_var", "num_glo_var");
    let ns_vars = read_names(reader, "name_ns_var", "num_ns_var");
    let ss_vars = read_names(reader, "name_ss_var", "num_ss_var");

    let eb_ids = read_id_array(reader, "eb_prop1", num_blk);
    let ns_ids = read_id_array(reader, "ns_prop1", num_ns);
    let ss_ids = read_id_array(reader, "ss_prop1", num_ss);

    let mut blocks = Vec::new();
    for i in 1..=num_blk {
        let num_elems = reader
            .meta
            .dim_len(&format!("num_el_in_blk{i}"))
            .unwrap_or(0);
        let nodes_per_elem = reader
            .meta
            .dim_len(&format!("num_nod_per_el{i}"))
            .unwrap_or(0);
        let elem_type = [
            format!("num_el_in_blk{i}"),
            format!("connect{i}"),
            "eb_prop1".to_string(),
        ]
        .iter()
        .find_map(|name| {
            reader
                .meta
                .var(name)
                .and_then(|v| v.attr("elem_type"))
                .and_then(|a| a.text().ok())
                .map(str::to_string)
        });
        blocks.push(Block {
            index: i,
            id: eb_ids.get(i - 1).copied().unwrap_or(i as i64),
            num_elems,
            nodes_per_elem,
            elem_type,
        });
    }

    let node_sets = (1..=num_ns)
        .map(|i| Set {
            index: i,
            id: ns_ids.get(i - 1).copied().unwrap_or(i as i64),
            count: reader.meta.dim_len(&format!("num_nod_ns{i}")).unwrap_or(0),
        })
        .collect();
    let side_sets = (1..=num_ss)
        .map(|i| Set {
            index: i,
            id: ss_ids.get(i - 1).copied().unwrap_or(i as i64),
            count: reader.meta.dim_len(&format!("num_side_ss{i}")).unwrap_or(0),
        })
        .collect();

    Ok(Summary {
        qa,
        num_dim,
        num_nodes,
        num_elems,
        blocks,
        node_sets,
        side_sets,
        times,
        nodal_vars,
        elem_vars,
        global_vars,
        ns_vars,
        ss_vars,
    })
}

fn read_qa(reader: &mut Reader<impl std::io::Read + std::io::Seek>) -> Result<Vec<String>, Error> {
    let Some(var) = reader.meta.var("qa_records").cloned() else {
        return Ok(Vec::new());
    };
    let values = reader.var_values(&var)?;
    let text: String = values.iter().map(|&v| char::from(v as u8)).collect();
    Ok(text
        .split('\0')
        .map(|s| s.trim_end_matches('\0'))
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect())
}

fn read_times(reader: &mut Reader<impl std::io::Read + std::io::Seek>) -> Option<Vec<f64>> {
    let var = reader.meta.var("time_whole").cloned()?;
    let mut times = Vec::new();
    reader
        .for_each_record(&var, |_, vals| times.push(vals[0]))
        .ok()?;
    Some(times)
}

/// Read a char-matrix name variable (`name_nod_var` and friends): rows of
/// `len_name` characters, one row per named entity.
fn read_names(
    reader: &mut Reader<impl std::io::Read + std::io::Seek>,
    var: &str,
    count_dim: &str,
) -> Vec<String> {
    let Some(var) = reader.meta.var(var).cloned() else {
        return Vec::new();
    };
    let count = reader.meta.dim_len(count_dim).unwrap_or(0);
    let Ok(values) = reader.var_values(&var) else {
        return Vec::new();
    };
    let width = values.len().checked_div(count).unwrap_or(0);
    (0..count)
        .map(|i| {
            values[i * width..(i + 1) * width]
                .iter()
                .map(|&v| char::from(v as u8))
                .collect::<String>()
                .trim_end_matches(['\0', ' '])
                .to_string()
        })
        .collect()
}

fn read_id_array(
    reader: &mut Reader<impl std::io::Read + std::io::Seek>,
    var: &str,
    count: usize,
) -> Vec<i64> {
    let Some(var) = reader.meta.var(var).cloned() else {
        return Vec::new();
    };
    reader
        .var_values(&var)
        .unwrap_or_default()
        .into_iter()
        .take(count)
        .map(|v| v as i64)
        .collect()
}

/// Min/max of one field of a record variable over all time steps, yielding
/// one `(time, min, max)` triple per record.
///
/// `field` selects the index along the first non-record dimension (the
/// variable index for `vals_nod_var`-style arrays, or the element index for
/// single-component arrays); use 0 for whole-record min/max.
pub fn field_stats(
    reader: &mut Reader<impl std::io::Read + std::io::Seek>,
    var_name: &str,
    field: usize,
) -> Result<Vec<(f64, f64, f64)>, Error> {
    let Some(var) = reader.meta.var(var_name).cloned() else {
        return Err(Error::MissingVar(var_name.to_string()));
    };
    let times = read_times(reader).unwrap_or_default();
    // Field stride: for arrays shaped (time, field, ...) the slab is the
    // trailing dimensions; for (time, length) arrays a field covers the
    // whole record.
    let slab: usize = if var.dimids.len() >= 3 {
        var.dimids
            .iter()
            .skip(2)
            .map(|&d| reader.meta.dims()[d].len.unwrap_or(1))
            .product::<usize>()
            .max(1)
    } else {
        usize::MAX
    };

    let mut out = Vec::new();
    reader.for_each_record(&var, |rec, vals| {
        let start = field * slab;
        if start >= vals.len() {
            return;
        }
        let end = (start + slab).min(vals.len());
        let mut lo = f64::INFINITY;
        let mut hi = f64::NEG_INFINITY;
        for &v in &vals[start..end] {
            if v.is_finite() {
                lo = lo.min(v);
                hi = hi.max(v);
            }
        }
        let t = times.get(rec).copied().unwrap_or(rec as f64);
        out.push((t, lo, hi));
    })?;
    Ok(out)
}
