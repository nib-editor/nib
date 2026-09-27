//! Subcommands that do not start the editor: `nib config ...` and
//! `nib plugin ...`.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use nib_core::Config;

use crate::builtin;
use crate::install::{self, Outcome, Store};
use crate::settings::{self, Source};

pub const USAGE: &str = "usage: nib [--plugin DIR]... [FILE]...
       nib --version                   show nib's version
       nib config path                 show where the settings are
       nib config init                 write commented settings files to start from
       nib plugin list                 list the plugins and their settings files
       nib plugin search [WORD]        find plugins to install by name
       nib plugin add SOURCE [--yes]   install by name[@tag], or from owner/repo[@tag], a URL, or a file
       nib plugin update [NAME]...     update installed plugins [--yes]
       nib plugin remove NAME          uninstall a plugin
       nib plugin pack DIR             make NAME-VERSION.nib.tar.gz from a built plugin
       nib plugin new NAME [DIR]       start a plugin in Rust, or with --go in Go
       nib plugin build [DIR]          build plugin.wasm with cargo or TinyGo
       nib plugin test [DIR] [FILE]... run a plugin's tests/*.toml without a terminal";

pub fn config(args: &[OsString]) -> ExitCode {
    let Some(dir) = settings::config_dir() else {
        eprintln!("nib: no home directory to keep settings in");
        return ExitCode::FAILURE;
    };
    let result = match args.first().and_then(|a| a.to_str()) {
        Some("path") => {
            show_paths(&dir);
            Ok(())
        }
        Some("init") => init(&dir),
        _ => Err(USAGE.to_string()),
    };
    finish(result)
}

pub fn plugin(args: &[OsString]) -> ExitCode {
    let args: Vec<&str> = args.iter().filter_map(|a| a.to_str()).collect();
    let yes = args.contains(&"--yes");
    let go = args.contains(&"--go");
    let words: Vec<&str> = args
        .iter()
        .copied()
        .filter(|a| !["--yes", "--go"].contains(a))
        .collect();
    let result = match words[..] {
        ["list"] => list(),
        ["search"] => search(""),
        ["search", word] => search(word),
        ["add", source] => with_store(|store| add(store, source, yes)),
        ["update", ref names @ ..] => with_store(|store| update(store, names, yes)),
        ["remove", name] => with_store(|store| remove(store, name)),
        ["pack", dir] => install::pack(Path::new(dir), Path::new("."))
            .map(|file| println!("wrote {}", file.display())),
        ["new", name] => new(name, None, go),
        ["new", name, dir] => new(name, Some(Path::new(dir)), go),
        ["build"] => crate::pluginbuild::run(Path::new(".")),
        ["build", dir] => crate::pluginbuild::run(Path::new(dir)),
        ["test", ref rest @ ..] => test(rest),
        _ => Err(USAGE.to_string()),
    };
    finish(result)
}

fn new(name: &str, dir: Option<&Path>, go: bool) -> Result<(), String> {
    let dir = crate::scaffold::run(name, dir, go)?;
    println!(
        "made {name} in {}; build and test it with\n\n  cd {}\n  nib plugin build\n  nib plugin test",
        dir.display(),
        dir.display()
    );
    Ok(())
}

/// `nib plugin test [DIR] [FILE]...`: the directory is the first word if
/// it is one, else the current one.
fn test(words: &[&str]) -> Result<(), String> {
    let (dir, files) = match words {
        [first, rest @ ..] if Path::new(first).is_dir() => (Path::new(*first), rest),
        files => (Path::new("."), files),
    };
    let files: Vec<PathBuf> = files.iter().map(PathBuf::from).collect();
    crate::plugintest::run(dir, &files)
}

fn with_store(f: impl FnOnce(&Store) -> Result<(), String>) -> Result<(), String> {
    let store = settings::store().ok_or("no home directory to install plugins in")?;
    f(&store)
}

/// What asks the user: nothing with `--yes`, the terminal otherwise.
fn confirmer(yes: bool) -> impl FnMut(&str) -> bool {
    move |question: &str| {
        if yes {
            println!("{question} yes (--yes)");
            true
        } else {
            install::ask(question)
        }
    }
}

fn search(word: &str) -> Result<(), String> {
    let listings = install::index()?;
    let found = install::search(&listings, word);
    if found.is_empty() {
        println!("no plugins found");
        return Ok(());
    }
    let width = |f: fn(&install::Listing) -> &str| found.iter().map(|l| f(l).len()).max();
    let name = width(|l| &l.name).unwrap_or(0).max("NAME".len());
    let source = width(|l| &l.source).unwrap_or(0).max("SOURCE".len());
    println!("{:name$}  {:source$}  DESCRIPTION", "NAME", "SOURCE");
    for listing in found {
        println!(
            "{:name$}  {:source$}  {}",
            listing.name, listing.source, listing.description
        );
    }
    Ok(())
}

fn add(store: &Store, text: &str, yes: bool) -> Result<(), String> {
    let builtin: Vec<&str> = builtin::PLUGINS.iter().map(|(name, _, _)| *name).collect();
    // A name is only a way to the source, which is what is kept.
    let (source, listed) = install::resolve(text)?;
    if let Some(name) = &listed {
        println!("{name} is {source} in the index");
    }
    match install::add(
        store,
        &source,
        listed.as_deref(),
        &builtin,
        &mut confirmer(yes),
    )? {
        Some(name) => println!(
            "installed {name} in {}; it loads the next time nib starts",
            store.dir(&name).display()
        ),
        None => println!("nothing installed"),
    }
    Ok(())
}

fn update(store: &Store, names: &[&str], yes: bool) -> Result<(), String> {
    let names: Vec<String> = if names.is_empty() {
        store.records()?.into_iter().map(|r| r.name).collect()
    } else {
        names.iter().map(|n| n.to_string()).collect()
    };
    if names.is_empty() {
        println!("no plugins are installed");
    }
    let mut failed = false;
    for name in names {
        match install::update(store, &name, &mut confirmer(yes)) {
            Ok(Outcome::Updated { from, to }) => println!("{name}: {from} -> {to}"),
            Ok(Outcome::UpToDate) => println!("{name}: up to date"),
            Ok(Outcome::Pinned) => {
                println!("{name}: installed from a tag or an archive; left as is")
            }
            Ok(Outcome::Declined) => println!("{name}: not updated"),
            Err(err) => {
                eprintln!("nib: {name}: {err}");
                failed = true;
            }
        }
    }
    if failed {
        Err("some plugins were not updated".into())
    } else {
        Ok(())
    }
}

fn remove(store: &Store, name: &str) -> Result<(), String> {
    install::remove(store, name)?;
    println!("removed {name}");
    let kept: Vec<PathBuf> = [
        settings::config_dir().map(|d| d.join("plugins").join(format!("{name}.toml"))),
        Some(store.data.join("plugins").join(name)),
    ]
    .into_iter()
    .flatten()
    .filter(|path| path.exists())
    .collect();
    for path in kept {
        println!("kept {}", path.display());
    }
    Ok(())
}

fn finish(result: Result<(), String>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("nib: {err}");
            ExitCode::FAILURE
        }
    }
}

fn show_paths(dir: &Path) {
    let config = dir.join("config.toml");
    let state = |exists: bool| if exists { "" } else { " (not created yet)" };
    println!("config:  {}{}", config.display(), state(config.is_file()));
    let plugins = dir.join("plugins");
    match settings::plugin_files(dir) {
        Ok(files) if !files.is_empty() => {
            println!("plugins: {}", plugins.display());
            for (_, path) in files {
                println!("         {}", path.display());
            }
        }
        _ => println!("plugins: {} (no files yet)", plugins.display()),
    }
    if let Some(cache) = settings::cache_dir() {
        println!("cache:   {}", cache.display());
    }
}

/// Writes the templates that do not exist yet, never overwriting a file.
fn init(dir: &Path) -> Result<(), String> {
    let plugins = dir.join("plugins");
    fs::create_dir_all(&plugins).map_err(|err| format!("{}: {err}", plugins.display()))?;
    let files = [
        (
            dir.join("config.toml"),
            settings::CONFIG_TEMPLATE.to_string(),
        ),
        (
            plugins.join("helix.toml"),
            settings::plugin_template("helix"),
        ),
    ];
    for (path, template) in files {
        if path.exists() {
            println!("kept    {} (already there)", path.display());
            continue;
        }
        fs::write(&path, template).map_err(|err| format!("{}: {err}", path.display()))?;
        println!("created {}", path.display());
    }
    Ok(())
}

fn list() -> Result<(), String> {
    let dir = settings::config_dir();
    let config = match &dir {
        Some(dir) => settings::load(dir)?,
        None => Config::default(),
    };
    let mut problems = Vec::new();
    let store = settings::store();
    let rows: Vec<[String; 4]> = settings::entries(&config, dir.as_deref(), store.as_ref())?
        .into_iter()
        .map(|entry| {
            let (source, problem) = match &entry.source {
                Source::Builtin(_) => ("built-in".to_string(), None),
                Source::Dir(path) => (
                    entry
                        .origin
                        .clone()
                        .unwrap_or_else(|| path.display().to_string()),
                    settings::check_name(&entry.name, path).err(),
                ),
            };
            let state = match (problem, entry.enabled) {
                (Some(problem), _) => {
                    problems.push(problem);
                    "error"
                }
                (None, true) => "enabled",
                (None, false) => "disabled",
            };
            let file = entry
                .file
                .map_or_else(|| "-".into(), |f| f.display().to_string());
            [entry.name, state.into(), source, file]
        })
        .collect();
    let header = ["NAME", "STATE", "SOURCE", "SETTINGS"].map(String::from);
    let widths: Vec<usize> = (0..3)
        .map(|i| {
            rows.iter()
                .chain([&header])
                .map(|row| row[i].len())
                .max()
                .unwrap_or(0)
        })
        .collect();
    for row in [&header].into_iter().chain(&rows) {
        println!(
            "{:w0$}  {:w1$}  {:w2$}  {}",
            row[0],
            row[1],
            row[2],
            row[3],
            w0 = widths[0],
            w1 = widths[1],
            w2 = widths[2],
        );
    }
    for problem in problems {
        println!("\nerror: {problem}");
    }
    Ok(())
}
