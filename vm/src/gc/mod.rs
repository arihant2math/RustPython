mod header;

pub use header::Header as PyGCHeader;

use crate::PyObjectRef;

pub enum Algorithm {
    MarkAndSweep,
    TriColor,
    None
}

impl Algorithm {
    fn mark_and_sweep(&self, roots: &[PyObjectRef]) {
        roots
    }

    fn tri_color(&self, roots: &[PyObjectRef]) {
        todo!()
    }

    pub fn execute(&self, roots: &[PyObjectRef]) {
        match self {
            Algorithm::MarkAndSweep => self.mark_and_sweep(roots),
            Algorithm::TriColor => self.tri_color(roots),
            Algorithm::None => {}
        }
    }
}
