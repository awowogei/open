#![feature(process_setsid)]

use std::{
    collections::HashMap,
    io::{BufRead, IsTerminal},
    path::{Path, PathBuf},
    str::FromStr,
    sync::LazyLock,
};

use mime::Mime;

use crate::desktop_entry::{DESKTOP_ENTRY_CACHE, DesktopEntry};

pub mod desktop_entry;

pub const XDG_DIRS: LazyLock<xdg_base_dirs::BaseDirectories> =
    // This can only fail when the home directory is missing
    LazyLock::new(|| {
        let Ok(base_dirs) = xdg_base_dirs::BaseDirectories::new() else {
            eprintln!("Missing $HOME directory");
            std::process::exit(1);
        };

        std::fs::create_dir_all(base_dirs.get_config_home()).ok();
        std::fs::create_dir_all(base_dirs.get_data_home()).ok();
        base_dirs
    });

/// Opens many paths with their default applications. If the paths use the same application and
/// the application supports it, they will be opened in the same window.
pub fn open(paths: Vec<String>) {
    let mut to_execute: HashMap<PathBuf, Vec<String>> = HashMap::new();

    for path in paths {
        let Some(mime_type) = get_mime_type(&path) else {
            eprintln!("Could not identify the mime type of '{}'", &path);
            continue;
        };

        let entry_path = match DesktopEntry::path_from_mimetype(&mime_type) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("{}", e);
                continue;
            }
        };

        to_execute.entry(entry_path).or_default().push(path);
    }

    // Only a single application can consume the terminal window, when one does so, other programs
    // that need a terminal has to launch their own.
    let mut can_consume_terminal = std::io::stdout().is_terminal();

    for (entry_path, paths) in to_execute {
        let desktop_entry = match DesktopEntry::load(&entry_path) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("{}", e);
                continue;
            }
        };
        desktop_entry.execute(&paths, &mut can_consume_terminal);
    }
}

/// Set an application as the default for the mimetype.
/// If the name does not correspond to the file name of a desktop entry, it will try its best to
/// find the one you're looking for.
pub fn set_default(application: &str, mimetype: &Mime) {
    let desktop_entry = match DesktopEntry::guess_from_name(application) {
        Ok(d) => d,
        Err(e) => {
            eprintln!(
                "{}",
                e.context(format!(
                    "Could not set the default application for {mimetype} to {application}"
                ))
            );
            return;
        }
    };

    let desktop_entry_file_name = desktop_entry.file_name.to_string_lossy().to_string();

    let mut mime_apps = MimeApps::load();
    mime_apps
        .defaults
        .insert(mimetype.clone(), vec![desktop_entry_file_name.clone()]);
    mime_apps.save();

    println!("{application} set as default application for {mimetype}");
}

/// Get the mime type of the input
pub fn get_mime_type(input: impl AsRef<str>) -> Option<Mime> {
    static DB: std::sync::OnceLock<xdg_mime::SharedMimeInfo> = std::sync::OnceLock::new();
    let mime_db = DB.get_or_init(|| xdg_mime::SharedMimeInfo::new());

    let input = input.as_ref();

    // paths can look exactly like mime/type, so do it first
    if Path::new(input).exists() {
        let guess = mime_db
            .guess_mime_type()
            // Don't return application/x-zerosize for empty file with extension
            .zero_size(false)
            .path(input)
            .guess();

        if *guess.mime_type() == Mime::from_str("application/x-zerosize").unwrap() {
            // Even if you just "touch new && open new" it should work even though you haven't
            // defined a handler for x-zerosize.
            return Mime::from_str("text/plain").ok();
        } else {
            return Some(guess.mime_type().clone());
        };
    } else if let Ok(mime) = input.parse::<Mime>() {
        // just parse what is already in mime/type format
        Some(mime)
    } else if let Some((protocol, _)) = input.split_once("://") {
        Some(
            format!("x-scheme-handler/{}", protocol)
                .parse::<Mime>()
                .unwrap(),
        )
    } else {
        // If all else fails, assume the user is trying to supply a file format.
        // If supplied as "pdf" it has to be changed to ".pdf" for the guess to work.
        let mut input = input.to_owned();
        if !input.starts_with('.') {
            input.insert(0, '.');
        };

        let guess = mime_db.guess_mime_type().file_name(&input).guess();
        if !guess.uncertain() {
            return Some(guess.mime_type().clone());
        } else {
            return None;
        }
    }
}

/// The mime types that have one or more applications registered as default
#[derive(Debug)]
pub struct MimeApps {
    pub defaults: HashMap<Mime, Vec<String>>,
    // TODO: This randomizes the order though, which is desired for vcs of dotfiles. Maybe use
    // indexmap
    //
    // These are stored so the file can be overwritten instead of having to edit the file with new
    // values. They are not used.
    _added_associations: HashMap<Mime, Vec<String>>,
    _removed_associations: HashMap<Mime, Vec<String>>,
}

impl MimeApps {
    // TODO: This gets loaded more than I thought, Oncelock it.
    pub fn load() -> Self {
        let mut mime_apps = Self {
            defaults: HashMap::new(),
            _added_associations: HashMap::new(),
            _removed_associations: HashMap::new(),
        };

        let _ = std::fs::create_dir(XDG_DIRS.get_config_home());

        let mut path = XDG_DIRS.get_config_home();
        path.push("mimeapps.list");

        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true) // Required for .create(true) to work
            .create(true) // Create file if it doesn't exist
            .open(path)
            .unwrap();
        let reader = std::io::BufReader::new(file);

        enum Section {
            None,
            Default,
            Added,
            Removed,
        }

        let mut section = Section::None;
        for line in reader.lines() {
            let Ok(line) = line else {
                // Shouldn't happen I think
                panic!("Failed to read mimeapps");
            };

            if line == "[Default Applications]" {
                section = Section::Default
            } else if line == "[Added Associations]" {
                section = Section::Added;
            } else if line == "[Removed Associations]" {
                section = Section::Removed;
            } else if line == "" {
                continue;
            }

            let Some((mime, desktop_entries)) = line.split_once("=") else {
                // Invalid entry, ignore
                continue;
            };
            let Ok(mime) = Mime::from_str(mime) else {
                // Invalid entry, ignore
                continue;
            };
            let desktop_entries = desktop_entries
                .split_terminator(";")
                .map(|str| str.to_owned())
                .collect();

            match section {
                Section::Default => {
                    mime_apps.defaults.insert(mime, desktop_entries);
                }
                Section::Added => {
                    mime_apps._added_associations.insert(mime, desktop_entries);
                }
                Section::Removed => {
                    mime_apps
                        ._removed_associations
                        .insert(mime, desktop_entries);
                }
                Section::None => (),
            };
        }

        return mime_apps;
    }

    fn save(&self) {
        let mut output = String::new();
        for (section, entries) in [
            ("[Default Applications]", &self.defaults),
            ("[Added Associations]", &self._added_associations),
            ("[Removed Associations]", &self._removed_associations),
        ] {
            output += section;
            output += "\n";
            for (mime_type, desktop_entries) in entries {
                output += mime_type.as_ref();
                output += "=";
                output += &desktop_entries.join(";");
                output += "\n";
            }
            if section != "[Removed Associations]" {
                output += "\n";
            }
        }

        let mut path = XDG_DIRS.get_config_home();
        path.push("mimeapps.list");

        if let Err(e) = std::fs::write(&path, &output) {
            eprintln!(
                "Failed to write default applications to {} with error: {}",
                &path.display(),
                e
            );
        }
    }
}

pub fn update_completions() {
    // Triggers lazy loading and writes to the cache file if it's outdated
    DESKTOP_ENTRY_CACHE.contains_key("");
}
