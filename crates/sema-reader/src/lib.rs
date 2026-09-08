#![allow(clippy::mutable_key_type)]
pub mod lexer;
mod reader;

pub use reader::ordered_map_literal;
pub use reader::ordered_map_literal_entries;
pub use reader::read;
pub use reader::read_many;
pub use reader::read_many_with_spans;
pub use reader::read_many_with_spans_recover;
pub use reader::read_many_with_symbol_spans;
