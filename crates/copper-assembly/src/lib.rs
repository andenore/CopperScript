//! Assembly locks and the BOM, hand-assembly and ordering lists
//! (`pcbir.assembly`), and native KiCad manufacturing files without
//! independent CAM signoff (`pcbir.manufacturing_files`).
//!
//! CSV files are written like Python's `csv` module (`"\r\n"` rows on every
//! platform) and archives like `zipfile` with zlib's default compression, so
//! every output file is byte-identical to the Python implementation's.

pub mod assembly;
pub mod csv;
pub mod deflate;
pub mod files;
pub mod manufacturing;
pub mod zip;
