use crate::layout::LayoutTree;
use cssparser::{Parser, ParserInput};

pub fn apply_css(layout_tree: &mut LayoutTree, css: &str) {
    let mut input = ParserInput::new(css);
    let mut parser = Parser::new(&mut input);
    
    // Very basic parsing for M5 compilation
    while let Ok(token) = parser.next() {
        match token {
            _ => {}
        }
    }
}
