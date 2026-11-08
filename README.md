# exo-inspect

Inspect [ExodusII](https://sandialabs.github.io/seacas-docs/sphinx/html/index.html) files
— the format [MOOSE](https://mooseframework.inl.gov) writes with `Outputs/exodus` —
from the command line, without ParaView.

## Why

On a remote HPC login node or in a CI job there is no ParaView, and `ncdump` speaks
netCDF, not Exodus. `exo-inspect` answers the usual questions directly: how many
nodes/elements, what blocks and element types, which sets exist, what variables are
stored, and how the field ranges evolve over time.

## Features

- Summary of dimensions, element blocks (IDs, element types, topology sizes), node
  sets, and side sets
- Time steps from `time_whole`, with `--times` for the full list
- Nodal / elemental / global / node-set / side-set variable names
- `--stats <var>` for per-time-step min/max of any record variable
  (e.g. `vals_nod_var1`)
- Pure-Rust classic netCDF reader (CDF-1 and CDF-2, the formats libMesh writes) with
  zero runtime dependencies — a single static binary. netCDF-4/HDF5 files are
  rejected with a clear error.

## Install

```sh
cargo install --git https://github.com/andrewwhitecdw/exo-inspect
```

## Usage

```sh
exo-inspect gold/diffusion_out.e
exo-inspect out.e --times
exo-inspect out.e --stats vals_nod_var1 --field 0
```

Example output:

```
gold/boundary_ic_out.e
  format:     CDF-2 (64-bit offset)
  dimensions: 2D, 25 nodes, 16 elements
  blocks:     1 (16 total elements)
    block 1 (id 0): 16 x QUAD4, 4 nodes/elem
  node sets:  4
    set 1 (id 0): 5 nodes
  side sets:  4
    set 1 (id 0): 4 sides
  time:       2 steps: 0, 1
  nodal variables: u
  elemental variables: none
```

## Relationship to MOOSE

MOOSE is developed by Idaho National Laboratory and licensed LGPL-2.1. This tool is an
independent MIT-licensed project that reads the files MOOSE produces; it contains no
MOOSE code.

## License

MIT
