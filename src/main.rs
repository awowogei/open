use clap::{Parser, Subcommand};
use std::{path::Path, process::ExitCode};

use open::desktop_entry::DesktopEntry;

const EXAMPLES: &str = r#"Examples:
  Open files:
    open image.png                -- Open a single file
    open *.pdf                    -- Open all pdf files

  Set default application:
    open --with firefox .pdf      -- Set default app for a filetype
    open --with firefox https:    -- Set default app for a protocol
    open -w firefox text/html     -- Set default app for a mimetype

  Query default applications:
    open --application text/html  -- Get the default app of a mimetype
    open -a picture.png           -- Get the default app of a file

  Query mimetypes:
    open --mimetype document.pdf  -- Get the mimetype of a file
"#;

#[derive(Parser)]
#[command(about, arg_required_else_help = true, subcommand_negates_reqs = true, after_help = EXAMPLES)]
struct Cli {
    #[command(subcommand)]
    command: Option<Hidden>,

    // A list of paths, urls or mimetypes
    #[clap(required = true)]
    inputs: Vec<String>,

    // Set the default application of the inputs, does not open them
    #[arg(short = 'w', long, value_name = "APPLICATION", group = "flag")]
    with: Option<String>,

    // Get the default applications of the inputs
    #[arg(short = 'a', long = "application", group = "flag")]
    get_application: bool,

    // Get the mimetypes of the inputs
    #[arg(short = 'm', long = "mimetype", group = "flag")]
    get_mime: bool,
}

#[derive(Subcommand)]
enum Hidden {
    // Update the shell completion cache at $XDG_CACHE_HOME/open/apps.
    #[command(hide = true)]
    UpdateDesktopEntryCache,
}

fn main() -> ExitCode {
    let args = Cli::parse();

    if let Some(Hidden::UpdateDesktopEntryCache) = args.command {
        open::update_completions();
        return ExitCode::SUCCESS;
    }

    if let Some(application) = args.with {
        for input in &args.inputs {
            let Some(mimetype) = open::get_mime_type(input) else {
                println!("Could not detect the mimetype of '{}'", input);
                continue;
            };

            open::set_default(&application, &mimetype);
        }
    } else if args.get_application {
        for path in args.inputs {
            let Some(mime_type) = open::get_mime_type(&path) else {
                eprintln!("'{path}' does not exist.");
                continue;
            };
            let path = match DesktopEntry::path_from_mimetype(&mime_type) {
                Ok(path) => path,
                Err(e) => {
                    eprintln!(
                        "{:?}",
                        e.context(format!("Failed to find application for {}", &path))
                    );
                    // eprintln!(
                    //     "{}",
                    //     e.context(format!("Failed to find application for {}, error:", &path))
                    // );
                    return ExitCode::FAILURE;
                }
            };

            match DesktopEntry::load(&path) {
                Ok(d) => println!("{}", &d.name),
                Err(e) => {
                    eprintln!(
                        "{}",
                        e.context(format!(
                            "Could not load desktop entry at {}, error:",
                            path.display()
                        ))
                    );
                    return ExitCode::FAILURE;
                }
            }
        }
    } else if args.get_mime {
        for path in args.inputs {
            if let Some(mime_type) = open::get_mime_type(&path) {
                println!("{}", mime_type);
            } else {
                println!("'{}' does not exist.", path);
            };
        }
    } else {
        open::open(args.inputs);
    }

    return ExitCode::SUCCESS;
}
