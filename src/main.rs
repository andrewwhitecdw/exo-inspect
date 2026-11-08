//! Command-line entry point for `exo-inspect`.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Parser;
use exo_inspect::netcdf::Reader;

#[derive(Parser)]
#[command(
    name = "exo-inspect",
    about = "Inspect ExodusII files without ParaView",
    version
)]
struct Cli {
    /// ExodusII file to inspect (classic netCDF; netCDF-4/HDF5 is not supported)
    file: PathBuf,

    /// Print every time step value (default shows at most 10)
    #[arg(long)]
    times: bool,

    /// Print per-time min/max for a variable (e.g. vals_nod_var), repeatable
    #[arg(long, value_name = "VAR")]
    stats: Vec<String>,

    /// Field index used with --stats (default 0)
    #[arg(long, default_value_t = 0)]
    field: usize,
}

fn main() -> Result<ExitCode> {
    let cli = Cli::parse();
    let mut reader =
        Reader::open(&cli.file).with_context(|| format!("opening {}", cli.file.display()))?;

    let summary = exo_inspect::exodus::summarize(&mut reader)?;
    let format = if reader.meta.is_64bit {
        "CDF-2 (64-bit offset)"
    } else {
        "CDF-1 (classic)"
    };
    println!("{}", cli.file.display());
    println!("  format:     {format}");
    if !summary.qa.is_empty() {
        println!("  qa:         {}", summary.qa.join(" | "));
    }
    println!(
        "  dimensions: {}D, {} nodes, {} elements",
        summary.num_dim, summary.num_nodes, summary.num_elems
    );

    print!("  blocks:     {}", summary.blocks.len());
    if summary.blocks.is_empty() {
        println!();
    } else {
        println!(" ({} total elements)", summary.num_elems);
        for b in &summary.blocks {
            let ty = b.elem_type.as_deref().unwrap_or("?");
            println!(
                "    block {} (id {}): {} x {ty}{}",
                b.index,
                b.id,
                b.num_elems,
                if b.nodes_per_elem > 0 {
                    format!(", {} nodes/elem", b.nodes_per_elem)
                } else {
                    String::new()
                }
            );
        }
    }

    println!("  node sets:  {}", summary.node_sets.len());
    for s in &summary.node_sets {
        println!("    set {} (id {}): {} nodes", s.index, s.id, s.count);
    }
    println!("  side sets:  {}", summary.side_sets.len());
    for s in &summary.side_sets {
        println!("    set {} (id {}): {} sides", s.index, s.id, s.count);
    }

    match &summary.times {
        Some(times) => {
            if times.is_empty() {
                println!("  time:       no time steps (steady)");
            } else if cli.times || times.len() <= 10 {
                println!(
                    "  time:       {} step{}: {}",
                    times.len(),
                    if times.len() == 1 { "" } else { "s" },
                    times
                        .iter()
                        .map(|t| format!("{t}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            } else {
                println!(
                    "  time:       {} steps, first = {}, last = {} (use --times for all)",
                    times.len(),
                    times.first().expect("non-empty"),
                    times.last().expect("non-empty")
                );
            }
        }
        None => println!("  time:       (no time_whole variable)"),
    }

    print_names("nodal variables", &summary.nodal_vars);
    print_names("elemental variables", &summary.elem_vars);
    print_names("global variables", &summary.global_vars);
    print_names("node-set variables", &summary.ns_vars);
    print_names("side-set variables", &summary.ss_vars);

    for var in &cli.stats {
        let rows = exo_inspect::exodus::field_stats(&mut reader, var, cli.field)
            .with_context(|| format!("reading variable '{var}'"))?;
        println!("  stats {var} (field {}):", cli.field);
        for (t, lo, hi) in rows {
            println!("    t = {t}: min = {lo:.6e}, max = {hi:.6e}");
        }
    }

    Ok(ExitCode::SUCCESS)
}

fn print_names(label: &str, names: &[String]) {
    if names.is_empty() {
        println!("  {label}: none");
    } else {
        println!("  {label}: {}", names.join(", "));
    }
}
