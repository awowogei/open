use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    str::FromStr,
    sync::LazyLock,
};

use anyhow::bail;
use freedesktop_entry_parser as fep;
use mime::Mime;

use crate::{MimeApps, XDG_DIRS};

// This is for command line completion so that programs can be mapped from the actual program
// you execute to a desktop entry, relieving the user of having to figure out the name of the
// desktop entry. e.g. you can type 'open --with nvim' instead of 'open --with org.neovim.nvim.desktop'
pub(crate) static DESKTOP_ENTRY_CACHE: LazyLock<HashMap<String, PathBuf>> = LazyLock::new(|| {
    let cache_mtime = std::fs::metadata(desktop_entry_cache_path())
        .and_then(|m| m.modified())
        .ok();

    let needs_regen = match cache_mtime {
        None => true,
        Some(cache_mtime) => desktop_entry_source_dirs().iter().any(|dir| {
            std::fs::metadata(dir)
                .and_then(|m| m.modified())
                .map(|src_mtime| src_mtime > cache_mtime)
                .unwrap_or(false)
        }),
    };

    if needs_regen {
        update_desktop_entry_cache()
    } else {
        let mut map = HashMap::new();

        let Ok(content) = std::fs::read_to_string(desktop_entry_cache_path()) else {
            eprintln!("Desktop entry cache missing, regenerating...");
            return update_desktop_entry_cache();
        };

        for line in content.lines() {
            let mut fields = line.split('\t');
            if let Some(exec) = fields.next()
                && fields.next().is_some() // name, only used by completions
                && let Some(path) = fields.next()
                && !exec.is_empty()
                && !path.is_empty()
            {
                map.insert(exec.to_owned(), PathBuf::from(path));
            } else {
                eprintln!("Desktop entry cache malformed, regenerating...");
                return update_desktop_entry_cache();
            }
        }
        map
    }
});

#[derive(Debug)]
pub struct DesktopEntry {
    pub name: String,
    pub file_name: OsString,
    pub no_display: bool,
    command: String,
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
            command: String::default(),
            no_display: false,
            use_terminal: false,
            categories: HashSet::new(),
        };

        // required keys
        for attribute in ["Exec", "Name"] {
            if let Some(value) = entry
                .section("Desktop Entry")
                .attr(attribute)
                .filter(|attr| attr.len() > 0)
            {
                match attribute {
                    "Exec" => desktop_entry.command = value.to_owned(),
                    "Name" => desktop_entry.name = value.to_owned(),
                    _ => (),
                }
            } else {
                bail!(
                    "Could not load '{}'\nMissing key: '{}'",
                    path.display(),
                    attribute
                );
            }
        }

        // optional keys
        for attribute in ["Terminal", "Categories", "NoDisplay"] {
            if let Some(value) = entry
                .section("Desktop Entry")
                .attr(attribute)
                .filter(|attr| attr.len() > 0)
            {
                match attribute {
                    "Categories" => {
                        desktop_entry.categories =
                            value.split(";").map(|cat| cat.to_owned()).collect();
                    }
                    "Terminal" => desktop_entry.use_terminal = value == "true",
                    "NoDisplay" => desktop_entry.no_display = value == "true",
                    _ => (),
                }
            }
        }

        return Ok(desktop_entry);
    }

    /// Get the path to the default desktop entry given a mime type
    pub fn path_from_mimetype(mime_type: &Mime) -> anyhow::Result<PathBuf> {
        let mimeapps = MimeApps::load();
        let Some(entry_names) = mimeapps.defaults.get(&mime_type).or_else(|| {
            // If an application for the specific mimetype cannot be found, fall back to the
            // wildcard mimetype, e.g. text/html => text/*
            let wildcard = format!("{}/*", mime_type.type_()).parse().unwrap();
            mimeapps.defaults.get(&wildcard)
        }) else {
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
                "Several default applications exist for {}, but none are installed.\n\
                    Install one of: {}",
                mime_type,
                entry_names.join(", ")
            );
        }
    }

    /// Try to guess an application's desktop entry given its name.
    pub fn try_from_name(name: &str) -> anyhow::Result<Self> {
        let mut desktop_filename = PathBuf::from(name);
        desktop_filename.set_extension("desktop");

        let Some(path) = DESKTOP_ENTRY_CACHE.get(name) else {
            bail!("No desktop entry exists for: {name}");
        };

        return DesktopEntry::load(path);
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

    pub fn execute(&self, input_arguments: &[String], consume_terminal: &mut bool) {
        let mut took_argument = false;
        let mut components = self.command.split_whitespace();

        let mut command = if self.use_terminal && !*consume_terminal {
            let Some(terminal) = get_terminal() else {
                eprintln!(
                    "Tried to open with {}, but it requires a terminal to run.\n\
                    Try to install the terminal you prefer, it will most likely be picked up automatically, or try one of:\n\
                    1. Run 'open --with YOUR_TERMINAL x-scheme-handler/terminal'\n\
                    2. Set the $TERMINAL environment variable to the name of the terminal executable\n",
                    &self.name
                );
                return;
            };

            let mut command = Command::new(&terminal.command);
            // Most(all?) terminals support -e for compatability since xterm had it.
            command.arg("-e");
            command
        } else {
            Command::new(components.next().unwrap())
        };

        for arg in components {
            match arg {
                "%F" | "%U" => {
                    command.args(input_arguments);
                    took_argument = true;
                }
                "%f" | "%u" => {
                    if input_arguments.len() > 1 {
                        for arg in input_arguments {
                            self.execute(std::slice::from_ref(arg), consume_terminal);
                        }
                        return;
                    } else {
                        command.arg(&input_arguments[0]);
                        took_argument = true;
                    }
                }
                _ => {
                    command.arg(arg);
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
            *consume_terminal = false;
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

    // TODO: Some apps don't use their executable name as the desktop entry file,
    // e.g. .../com.system76.CosmicFiles.desktop -> executable name: cosmic-files -> desired: cosmic-files
    // and some use non-standard executable names in their commands,
    // e.g. .../gimp.desktop -> executable_name: gimp-3.2 -> desired: gimp
    // So there is seeminly no good way to get the executable name as you would write it in the
    // terminal...
    // Opting for extracting it from the command for now as it looks like it yields the best
    // results.
    pub fn executable_name(&self) -> Option<&str> {
        let mut substitution_found = false;
        for component in self.command.split_whitespace().rev() {
            if !substitution_found && matches!(component, "%f" | "%F" | "%u" | "%U") {
                substitution_found = true;
                continue;
            }
            if substitution_found && !component.starts_with("-") {
                let trimmed = component.trim_matches(|c| c == '"' || c == '\'');
                return Some(trimmed.rsplit('/').next().unwrap());
            }
        }

        return None;
    }
}

fn desktop_entry_source_dirs() -> [PathBuf; 3] {
    [
        XDG_DIRS.get_data_home().join("applications"),
        PathBuf::from("/usr/share/applications"),
        PathBuf::from("/usr/local/share/applications"),
    ]
}

fn desktop_entry_cache_path() -> PathBuf {
    let mut p = XDG_DIRS.get_cache_home();
    p.push("open");
    p.push("apps");
    return p;
}

// Write all desktop entries that are executable with one or more arguments to a cache file.
fn update_desktop_entry_cache() -> HashMap<String, PathBuf> {
    let mut entries: HashMap<String, (String, PathBuf)> = HashMap::new();
    for path in XDG_DIRS.list_data_files_once("applications") {
        let Ok(entry) = DesktopEntry::load(&path) else {
            continue;
        };
        if entry.no_display {
            continue;
        }
        let Some(executable) = entry.executable_name() else {
            continue;
        };

        entries.insert(executable.to_owned(), (entry.name, path));
    }

    let cache_path = desktop_entry_cache_path();
    if let Some(parent) = cache_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let mut buf = String::new();
    let mut map: HashMap<String, PathBuf> = HashMap::with_capacity(entries.len());
    for (exec, (name, path)) in entries {
        buf.push_str(&exec);
        buf.push('\t');
        buf.push_str(&name);
        buf.push('\t');
        buf.push_str(&path.to_string_lossy());
        buf.push('\n');
        map.insert(exec, path);
    }

    if let Err(e) = std::fs::write(&cache_path, &buf) {
        eprintln!(
            "Failed to write desktop entry cache to {}: {}",
            cache_path.display(),
            e
        );
    }

    return map;
}

fn get_terminal() -> Option<DesktopEntry> {
    match DesktopEntry::path_from_mimetype(&Mime::from_str("x-scheme-handler/terminal").unwrap()) {
        Ok(path) => match DesktopEntry::load(&path) {
            Ok(d) => return Some(d),
            Err(e) => {
                eprintln!(
                    "{:?}",
                    e.context(format!(
                        "Could not load desktop entry at {}",
                        path.display()
                    ))
                );
                return None;
            }
        },
        Err(_) => (),
    }

    if let Ok(terminal) = std::env::var("TERMINAL") {
        DesktopEntry::try_from_name(&terminal).ok()
    } else {
        DesktopEntry::guess_from_category("TerminalEmulator")
    }
}
