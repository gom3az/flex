//! Smallest useful menu: one tab, two rows.
//!
//! Needs a real terminal to *run* (`cargo run --example minimal`); the CI gate
//! only builds it.

use flex_core::{Menu, Row, RowId, Tab};

fn main() -> anyhow::Result<()> {
    let rows = vec![
        Row::with_meta(RowId::new("first"), "First choice", "right-hand meta"),
        Row::with_meta(RowId::new("second"), "Second choice", "right-hand meta"),
    ];
    let menu = Menu::new("example", vec![Tab::with_rows("Items", rows)]);
    let outcome = futures::executor::block_on(flex_core::run::run_capture(menu))?;
    println!("{outcome:?}");
    Ok(())
}
