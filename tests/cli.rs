//! End-to-end tests: build a synthetic CDF-1 Exodus-like file on disk and
//! run the binary against it.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// Little classic-netCDF writer, just enough for the test fixture.
///
/// Layout is built in two passes: first the header with placeholder begin
/// offsets (positions recorded), then the offsets are patched once the
/// header length is known.
struct Writer {
    dims: Vec<(String, u32)>,
    vars: Vec<TestVar>,
}

struct TestVar {
    name: String,
    dimids: Vec<usize>,
    nctype: u32,
    values: Vec<u8>,
    elem_size: usize,
    atts: Vec<(String, Vec<u8>)>,
}

impl TestVar {
    /// Size of one record slab (unlimited dimensions contribute one slot),
    /// padded to a 4-byte boundary as the format requires.
    fn padded_size(&self, dims: &[(String, u32)]) -> usize {
        let mut n = 1usize;
        for &d in &self.dimids {
            let len = dims[d].1;
            n *= if len == 0 { 1 } else { len as usize };
        }
        let raw = n * self.elem_size;
        raw.div_ceil(4) * 4
    }

    /// Total on-disk data size: record variables store `numrecs` slabs.
    fn data_size(&self, dims: &[(String, u32)], numrecs: u32) -> usize {
        let records = if dims[self.dimids[0]].1 == 0 {
            numrecs.max(1) as usize
        } else {
            1
        };
        records * self.padded_size(dims)
    }
}

fn push_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_be_bytes());
}

fn push_name(buf: &mut Vec<u8>, s: &str) {
    push_u32(buf, s.len() as u32);
    buf.extend_from_slice(s.as_bytes());
    buf.resize(buf.len() + (4 - s.len() % 4) % 4, 0);
}

fn build(w: &Writer) -> Vec<u8> {
    let mut header = Vec::new();
    header.extend_from_slice(b"CDF\x01");
    push_u32(&mut header, 2); // two records
    // dims
    push_u32(&mut header, 0x0A);
    push_u32(&mut header, w.dims.len() as u32);
    for (name, len) in &w.dims {
        push_name(&mut header, name);
        push_u32(&mut header, *len);
    }
    // global atts: none
    push_u32(&mut header, 0);
    push_u32(&mut header, 0);
    // vars
    push_u32(&mut header, 0x0B);
    push_u32(&mut header, w.vars.len() as u32);
    let mut begin_positions = Vec::new();
    for var in &w.vars {
        push_name(&mut header, &var.name);
        push_u32(&mut header, var.dimids.len() as u32);
        for &d in &var.dimids {
            push_u32(&mut header, d as u32);
        }
        if var.atts.is_empty() {
            push_u32(&mut header, 0);
            push_u32(&mut header, 0);
        } else {
            push_u32(&mut header, 0x0C);
            push_u32(&mut header, var.atts.len() as u32);
            for (name, value) in &var.atts {
                push_name(&mut header, name);
                push_u32(&mut header, 2); // char
                push_u32(&mut header, value.len() as u32);
                header.extend_from_slice(value);
                header.resize(header.len() + (4 - value.len() % 4) % 4, 0);
            }
        }
        push_u32(&mut header, var.nctype);
        push_u32(&mut header, var.padded_size(&w.dims) as u32);
        begin_positions.push(header.len());
        push_u32(&mut header, 0); // begin placeholder
    }

    let mut offset = header.len() as u32;
    for (pos, var) in begin_positions.iter().zip(&w.vars) {
        header[*pos..*pos + 4].copy_from_slice(&offset.to_be_bytes());
        offset += var.data_size(&w.dims, 2) as u32;
    }

    let mut file = header;
    for var in &w.vars {
        let size = var.data_size(&w.dims, 2);
        file.extend_from_slice(&var.values);
        file.resize(file.len() + size - var.values.len(), 0);
    }
    file
}

fn f64s(vals: &[f64]) -> Vec<u8> {
    vals.iter().flat_map(|v| v.to_be_bytes()).collect()
}

fn fixture(name: &str) -> PathBuf {
    let w = Writer {
        dims: vec![
            ("time_step".into(), 0),
            ("num_dim".into(), 2),
            ("num_nodes".into(), 4),
            ("num_elem".into(), 2),
            ("num_el_blk".into(), 1),
            ("num_node_sets".into(), 1),
            ("num_side_sets".into(), 1),
            ("num_nod_var".into(), 1),
            ("len_name".into(), 8),
            ("num_el_in_blk1".into(), 2),
            ("num_nod_per_el1".into(), 4),
            ("num_nod_ns1".into(), 2),
            ("num_side_ss1".into(), 3),
        ],
        vars: vec![
            TestVar {
                name: "time_whole".into(),
                dimids: vec![0],
                nctype: 6,
                values: f64s(&[0.0, 1.0]),
                elem_size: 8,
                atts: vec![],
            },
            TestVar {
                name: "coordx".into(),
                dimids: vec![2],
                nctype: 6,
                values: f64s(&[0.0, 1.0, 0.0, 1.0]),
                elem_size: 8,
                atts: vec![],
            },
            TestVar {
                name: "eb_prop1".into(),
                dimids: vec![4],
                nctype: 4,
                values: 7i32.to_be_bytes().to_vec(),
                elem_size: 4,
                atts: vec![("elem_type".into(), b"QUAD4".to_vec())],
            },
            TestVar {
                name: "name_nod_var".into(),
                dimids: vec![8, 7],
                nctype: 2,
                values: {
                    let mut v = b"u".to_vec();
                    v.resize(8, 0);
                    v
                },
                elem_size: 1,
                atts: vec![],
            },
            TestVar {
                name: "ns_prop1".into(),
                dimids: vec![5],
                nctype: 4,
                values: 9i32.to_be_bytes().to_vec(),
                elem_size: 4,
                atts: vec![],
            },
            TestVar {
                name: "ss_prop1".into(),
                dimids: vec![6],
                nctype: 4,
                values: 10i32.to_be_bytes().to_vec(),
                elem_size: 4,
                atts: vec![],
            },
        ],
    };
    let bytes = build(&w);
    let path = std::env::temp_dir().join(format!("exo-inspect-{name}-{}.e", std::process::id()));
    fs::write(&path, bytes).expect("write fixture");
    path
}

#[test]
fn summarizes_a_minimal_exodus_file() {
    let file = fixture("summary");
    let output = Command::new(env!("CARGO_BIN_EXE_exo-inspect"))
        .arg(&file)
        .output()
        .expect("run binary");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(stdout.contains("CDF-1 (classic)"), "{stdout}");
    assert!(stdout.contains("2D, 4 nodes, 2 elements"), "{stdout}");
    assert!(
        stdout.contains("block 1 (id 7): 2 x QUAD4, 4 nodes/elem"),
        "{stdout}"
    );
    assert!(stdout.contains("2 steps: 0, 1"), "{stdout}");
    assert!(stdout.contains("nodal variables: u"), "{stdout}");
    assert!(stdout.contains("set 1 (id 9): 2 nodes"), "{stdout}");
    assert!(stdout.contains("set 1 (id 10): 3 sides"), "{stdout}");
    fs::remove_file(&file).ok();
}

#[test]
fn stats_flag_reports_min_max_per_step() {
    let file = fixture("stats");
    let output = Command::new(env!("CARGO_BIN_EXE_exo-inspect"))
        .arg(&file)
        .arg("--stats")
        .arg("time_whole")
        .output()
        .expect("run binary");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(stdout.contains("stats time_whole"), "{stdout}");
    assert!(stdout.contains("t = 1: min = 1.000000e0"), "{stdout}");
    fs::remove_file(&file).ok();
}

#[test]
fn rejects_hdf5_files_clearly() {
    let path = std::env::temp_dir().join(format!("exo-inspect-h5-{}.e", std::process::id()));
    fs::write(&path, b"\x89HDF\r\n\x1a\nrest of file").expect("write");
    let output = Command::new(env!("CARGO_BIN_EXE_exo-inspect"))
        .arg(&path)
        .output()
        .expect("run binary");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not a netCDF file"), "{stderr}");
    fs::remove_file(&path).ok();
}
