//! CLI of spike OF-4.1.
//!
//!   sigil-syntax-proto check [--dir <import dir>] <file>...  diagnostics as a modder sees them
//!   sigil-syntax-proto set <file> <path> <value>             lossless edit, prints the new text
//!   sigil-syntax-proto report [--write]                      all measurements (Markdown);
//!                                                            --write refreshes ../results.md

use sigil_syntax_proto::{check, diag, measure, ron_cst, sigil};
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("check") => {
            let (dir, files) = match args.get(1).map(String::as_str) {
                Some("--dir") if args.len() > 2 => (Some(args[2].clone()), &args[3..]),
                _ => (None, &args[1..]),
            };
            for f in files {
                let out = match &dir {
                    Some(d) => check::check_file_in(Path::new(f), Path::new(d)),
                    None => check::check_file(Path::new(f)),
                };
                if out.diags.is_empty() {
                    println!("{f}: ok");
                }
                for d in &out.diags {
                    print!("{}", diag::render(f, &out.src, d));
                    if let Some(raw) = &d.raw {
                        println!("  = raw: {raw}");
                    }
                }
            }
        }
        Some("set") if args.len() == 4 => {
            let src = std::fs::read_to_string(&args[1]).expect("read file");
            let r = if args[1].ends_with(".ron") {
                ron_cst::set(&src, &args[2], &args[3])
            } else {
                sigil::set(&src, &args[2], &args[3])
            };
            match r {
                Ok(s) => print!("{s}"),
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                }
            }
        }
        Some("report") => {
            if args.get(1).map(String::as_str) == Some("--write") {
                let path = measure::results_path();
                let text = std::fs::read_to_string(&path).expect("results.md");
                let updated = measure::update_results(&text, &measure::blocks());
                std::fs::write(&path, updated).expect("write results.md");
                println!("updated {}", path.display());
            } else {
                print!("{}", measure::full_report());
            }
        }
        _ => {
            eprintln!(
                "usage: check [--dir <d>] <file>... | set <file> <path> <value> | report [--write]"
            );
            std::process::exit(2);
        }
    }
}
