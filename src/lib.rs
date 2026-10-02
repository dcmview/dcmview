/// `println!` for status lines. A closed stdout (`dcmview ... | head -n 1`, or
/// a launcher that stopped reading) is ignored instead of panicking, so the
/// viewer keeps serving and exits normally.
#[macro_export]
macro_rules! status_line {
    ($($arg:tt)*) => {{
        use ::std::io::Write as _;
        let _ = ::std::writeln!(::std::io::stdout(), $($arg)*);
    }};
}

pub mod annotations;
pub mod api;
mod dicom_values;
pub mod geometry;
pub mod loader;
pub mod masking;
pub mod object_kind;
pub mod pixels;
pub mod plane_stack;
pub mod presentation_state;
pub mod references;
pub mod semantic;
pub mod series;
pub mod server;
pub mod signals;
pub mod types;
pub mod value_mapping;
pub mod wsi;
