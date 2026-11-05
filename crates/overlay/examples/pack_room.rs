//! Packs a captured room (a folder of captures, with its `ambience.ogg` if it has one) into one `.monakaroom` file ([`monaka_overlay::scene::pack`]), and says what it came to.
//!
//! ```text
//! cargo run --release -p monaka_overlay --example pack_room -- <capture folder> <out.monakaroom>
//! ```

use monaka_overlay::scene::pack::{self, WHOLE};
use std::path::PathBuf;
use std::time::Instant;

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [folder, out] = args.as_slice() else {
        return Err("give: <capture folder> <out.monakaroom>".into());
    };
    let started = Instant::now();
    let (bytes, report) = pack::pack_folder(&PathBuf::from(folder))?;
    std::fs::write(out, &bytes).map_err(|e| format!("{out}: {e}"))?;
    let mb = |n: usize| n as f64 / 1e6;
    for (name, images, lists, kinds) in &report.places {
        let whole = kinds.iter().filter(|k| **k == WHOLE).count();
        println!("  {name}: images {:.2} MB ({whole} hours whole, {} as changes), depth and lists {:.2} MB", mb(*images), kinds.len() - whole, mb(*lists));
    }
    println!("  sound: {:.2} MB", mb(report.sound));
    println!("{out}: {:.2} MB, packed in {:.0} s", mb(report.total), started.elapsed().as_secs_f64());
    Ok(())
}
