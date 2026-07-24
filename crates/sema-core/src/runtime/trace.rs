use crate::cycle::GcEdge;

pub trait Trace {
    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool;
}
