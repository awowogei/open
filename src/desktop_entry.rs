use std::{
    collections::HashSet,
    ffi::OsString,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    str::FromStr,
};

use crate::XDG_DIRS;
use anyhow::bail;
use freedesktop_entry_parser as fep;
use mime::Mime;

#[derive(Debug)]
pub struct DesktopEntry {
    pub name: String,
    pub file_name: OsString,
    executable: String,
    exec_args: Vec<String>,
    use_terminal: bool,
    categories: HashSet<String>,
}

impl DesktopEntry {
    /// Load a desktop entry file.
    pub fn load(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();

        let entry = fep::parse_entry(&path)?;
        let mut desktop_entry = DesktopEntry {
            name: String::default(),
            file_name: path.file_name().unwrap().to_owned(),
            executable: String::default(),
            exec_args: Vec::new(),
            use_terminal: false,
            categories: HashSet::new(),
        };

        for attribute in ["Exec", "Name", "Terminal", "Categories"] {
            if let Some(value) = entry
                .section("Desktop Entry")
                .attr(attribute)
                .filter(|attr| attr.len() > 0)
            {
                match attribute {
                    "Exec" => {
                        let mut components = value.split_whitespace();
                        // TODO: This unwrap needs to be validated in case the exec field is malformed.
                        desktop_entry.executable = components.next().unwrap().to_owned();
                        desktop_entry.exec_args = components.map(|s| s.to_owned()).collect();
                    }
                    "Name" => desktop_entry.name = value.to_owned(),
                    "Categories" => {
                        desktop_entry.categories =
                            value.split(";").map(|cat| cat.to_owned()).collect();
                    }
                    "Terminal" => desktop_entry.use_terminal = value == "true",
                    _ => (),
                }
            } else {
                eprintln!(
                    "Malformed desktop entry at {}, missing attribute '{}'",
                    path.display(),
                    attribute
                );
                std::process::exit(1);
            }
        }

        return Ok(desktop_entry);
    }

    /// Get the path to the default desktop entry given a mime type
    pub fn path_from_mimetype(mime_type: &Mime) -> anyhow::Result<PathBuf> {
        let mimeapps = crate::MimeApps::load();
        let Some(entry_names) = mimeapps.defaults.get(&mime_type) else {
            bail!(
                "No application set as default for mime type: {mime_type}\n\
                   Set one with 'open --with YOUR_PROGRAM {mime_type}'"
            );
        };

        for filename in entry_names {
            if let Some(path) = XDG_DIRS.find_data_file(format!("applications/{filename}")) {
                return Ok(path);
            }
        }

        if entry_names.len() == 1 {
            bail!(
                "{} is the default application for {}, but it does not seem to be installed",
                entry_names[0],
                mime_type,
            );
        } else {
            bail!(
                "Several default applications exist for mime type {}, but none are installed.\n\
                    Install one of: {}",
                mime_type,
                entry_names.join(", ")
            );
        }
    }

    /// Try to guess an application's desktop entry given its name.
    pub fn guess_from_name(name: &str) -> anyhow::Result<Self> {
        let mut desktop_filename = PathBuf::from(name);
        desktop_filename.set_extension("desktop");

        if let Some(path) =
            XDG_DIRS.find_data_file(&format!("applications/{}", desktop_filename.display()))
        {
            return DesktopEntry::load(path);
        }

        // To make sure we only match against exact matches of commands.
        for path in XDG_DIRS.list_data_files_once("applications") {
            let Ok(desktop_entry) = DesktopEntry::load(&path) else {
                continue;
            };

            if &desktop_entry.name == &desktop_entry.name.to_lowercase()
                || desktop_entry.executable == name
            {
                return DesktopEntry::load(path);
            }
        }

        bail!("Could not find an application named {}", name);
    }

    // Try to guess an application's desktop entry given an application category.
    pub fn guess_from_category(category: &str) -> Option<Self> {
        for path in XDG_DIRS.list_data_files_once("applications") {
            let Ok(desktop_entry) = DesktopEntry::load(&path) else {
                continue;
            };

            if desktop_entry.categories.contains(category) {
                return Some(desktop_entry);
            } else {
                return None;
            }
        }

        return None;
    }

    pub fn execute(&self, input_arguments: Vec<String>, consume_terminal: &mut bool) {
        let mut took_argument = false;

        let mut command = if self.use_terminal {
            if *consume_terminal {
                Command::new(&self.executable)
            } else {
                let Some(terminal) = get_terminal() else {
                    eprintln!(
                        "Tried to open {}, but it requires a terminal to run.\n\
                        Try to install the terminal you prefer, it will most likely try to use it automatically, or try one of:\n\
                        Run 'open --with YOUR_TERMINAL x-scheme-handler/terminal'\n\
                        Set the $TERMINAL environment variable to the name of the terminal\n",
                        &self.name
                    );
                    return;
                };

                let mut command = Command::new(&terminal.executable);
                // Most(all?) terminals support -e for compatability since xterm had it.
                command.arg("-e");
                command.arg(&self.executable);
                command
            }
        } else {
            Command::new(&self.executable)
        };

        for exec_argument in &self.exec_args {
            match exec_argument.as_str() {
                "%F" | "%U" => {
                    command.args(&input_arguments);
                    took_argument = true;
                }
                "%f" | "%u" => {
                    if input_arguments.len() > 1 {
                        for arg in input_arguments {
                            self.execute(vec![arg], consume_terminal);
                        }
                        return;
                    } else {
                        command.arg(&input_arguments[0]);
                        took_argument = true;
                    }
                }
                _ => {
                    command.arg(exec_argument);
                }
            }
        }

        // If the desktop entry exec key doesn't explicitly allow arguments, we try anyway.
        if !took_argument {
            command.args(input_arguments);
        }

        if self.use_terminal && *consume_terminal {
            // Replaces the "open" process so the command can take control of the terminal
            let _ = command.exec();
        } else {
            // All other processes are detached so that they keep running even though the process
            // that owns them has exited.
            command
                .setsid(true)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
        }
    }
}

fn get_terminal() -> Option<DesktopEntry> {
    match DesktopEntry::path_from_mimetype(&Mime::from_str("x-scheme-handler/terminal").unwrap()) {
        Ok(path) => match DesktopEntry::load(&path) {
            Ok(d) => return Some(d),
            Err(e) => {
                eprintln!(
                    "{}",
                    e.context(format!(
                        "Could not load desktop entry at {}, error:",
                        path.display()
                    ))
                );
                return None;
            }
        },
        Err(_) => (),
    }

    if let Ok(terminal) = std::env::var("TERMINAL") {
        DesktopEntry::guess_from_name(&terminal).ok()
    } else {
        DesktopEntry::guess_from_category("TerminalEmulator")
    }
}
