#![feature(command_access)]

use getopts::Options;
use log::{error, warn};
use serde::Deserialize;
use serde_yaml::from_reader;
use std::collections::hash_map::{DefaultHasher, RandomState};
use std::collections::HashMap;
use std::fs::{self, File};
use std::hash::{BuildHasher, Hash, Hasher};
use std::io::{self, Cursor, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::{env, thread};
use tar::{Archive, Entry};

fn main() {
    env_logger::init();
    let opts = set_opts();
    let args: Vec<String> = env::args().collect();
    let params = read_params(&opts, &args);
    if params.help {
        print!("{}", opts.usage("Usage: factory [options]"));
    } else if let Some(cpath) = params.config {
        let cfile = File::open(cpath).unwrap();
        let conf: Config = from_reader(cfile).unwrap();
        if let Some(ipath) = params.input {
            let ifile = File::open(ipath).unwrap();
            let mut archive = Archive::new(ifile);
            process_files(&conf, archive.entries().unwrap(), |_| Ok(io::stdout())).unwrap();
        }
    }
}

fn set_opts() -> Options {
    let mut opts = Options::new();
    opts.optflag("h", "help", "Show this help information.");
    opts.optopt("c", "config", "Path to the config file", "PATH");
    opts.optopt("i", "input", "Path to the input file", "PATH");
    opts
}

fn read_params(opts: &Options, args: &Vec<String>) -> Params {
    let matches = opts.parse(&args[1..]).unwrap();
    Params {
        help: matches.opt_present("help"),
        config: matches.opt_get("config").unwrap(),
        input: matches.opt_get("input").unwrap(),
    }
}

struct Params {
    help: bool,
    config: Option<PathBuf>,
    input: Option<PathBuf>,
}

fn process_files<
    T: Iterator<Item = io::Result<U>>,
    U: Read + Name,
    V: Fn(String) -> io::Result<W>,
    W: Write + Send + 'static,
>(
    conf: &Config,
    iter: T,
    output: V,
) -> io::Result<()> {
    let mut gen = Gen::new();
    let mut buf = Vec::with_capacity(1024);
    for entry_result in iter {
        let mut entry = entry_result?;
        let name = entry.name()?;
        buf.clear();
        (&mut entry).take(1024).read_to_end(&mut buf)?;
        match get_file_type(&name, &buf) {
            Some(f) => match conf.get(&f) {
                Some(p) => {
                    let cur = Cursor::new(&mut buf);
                    let proc = prep_process(&mut gen, p);
                    run_process(proc, cur.chain(entry), output(f)?)?;
                }
                None => error!("File type not included in config: {}", f),
            },
            None => warn!("File type not determined for {}", name),
        }
    }
    Ok(())
}

type Config = HashMap<FileType, Plugin>;

type FileType = String;

#[derive(Debug, Deserialize)]
struct Plugin {
    name: String,
    path: PathBuf,
    args: Option<Vec<String>>,
    stdin: Option<bool>,
    stdout: Option<bool>,
}

fn get_file_type(_name: &str, head: &[u8]) -> Option<FileType> {
    if &head[..4] == b"FILE" {
        Some("ntfs.mft".into())
    } else if &head[..9] == b"#!/bin/sh" {
        Some("script.sh".into())
    } else {
        None
    }
}

trait Name {
    fn name(&self) -> io::Result<String>;
}

impl<T: Read> Name for Entry<'_, T> {
    fn name(&self) -> io::Result<String> {
        let path = self.path()?;
        Ok(String::from(path.file_name().unwrap().to_str().unwrap()))
    }
}

fn prep_process(gen: &mut Gen, plugin: &Plugin) -> PreppedProcess {
    let mut cmd = Command::new(&plugin.path);
    let mut args = plugin.args.clone().unwrap_or(Vec::new());
    let input_file_name = if plugin.stdin.unwrap_or(false) {
        cmd.stdin(Stdio::piped());
        None
    } else {
        cmd.stdin(Stdio::null());
        let input_name = gen.gen_string();
        cmd.env("INPUT", &input_name);
        replace_arg(&mut args, "$INPUT", &input_name);
        Some(input_name)
    };
    let output_file_name = if plugin.stdout.unwrap_or(false) {
        None
    } else {
        let output_name = gen.gen_string();
        cmd.env("OUTPUT", &output_name);
        replace_arg(&mut args, "$OUTPUT", &output_name);
        Some(output_name)
    };
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    PreppedProcess {
        command: cmd,
        input_file_name,
        output_file_name,
    }
}

struct PreppedProcess {
    command: Command,
    input_file_name: Option<String>,
    output_file_name: Option<String>,
}

struct Gen(u64, String, DefaultHasher);

impl Gen {
    fn new() -> Gen {
        Gen(
            0,
            String::with_capacity(16),
            RandomState::new().build_hasher(),
        )
    }

    fn gen_str(&mut self) -> &str {
        self.0.hash(&mut self.2);
        self.0 = self.2.finish();
        self.1 = format!("{:016x}", self.0);
        &self.1
    }

    fn gen_string(&mut self) -> String {
        self.gen_str().to_string()
    }
}

fn replace_arg(args: &mut Vec<String>, var: &str, rep: &str) {
    let idxs = args
        .iter()
        .enumerate()
        .filter(|x| x.1 == var)
        .map(|x| x.0)
        .collect::<Vec<usize>>();
    for idx in idxs {
        args.remove(idx);
        args.insert(idx, rep.to_string());
    }
}

fn run_process<T: Read, U: Write + Send + 'static>(
    mut proc: PreppedProcess,
    mut input: T,
    mut output: U,
) -> io::Result<()> {
    match (proc.input_file_name, proc.output_file_name) {
        (Some(i), Some(o)) => {
            let mut input_file = File::create(&i)?;
            io::copy(&mut input, &mut input_file)?;
            File::create(&o)?;
            let mut child = proc.command.spawn()?;
            child.wait()?;
            let mut output_file = File::open(&o)?;
            io::copy(&mut output_file, &mut output)?;
            fs::remove_file(i)?;
            fs::remove_file(o)?;
        }
        (Some(i), None) => {
            let mut input_file = File::create(&i)?;
            io::copy(&mut input, &mut input_file)?;
            let mut child = proc.command.spawn()?;
            let mut stdout = child.stdout.take().unwrap();
            io::copy(&mut stdout, &mut output)?;
            child.wait()?;
            fs::remove_file(i)?;
        }
        (None, Some(o)) => {
            File::create(&o)?;
            let mut child = proc.command.spawn()?;
            let mut stdin = child.stdin.take().unwrap();
            io::copy(&mut input, &mut stdin)?;
            child.wait()?;
            let mut output_file = File::open(&o)?;
            io::copy(&mut output_file, &mut output)?;
            fs::remove_file(o)?;
        }
        (None, None) => {
            let mut child = proc.command.spawn()?;
            let mut stdout = child.stdout.take().unwrap();
            let th = thread::spawn(move || io::copy(&mut stdout, &mut output).unwrap());
            let mut stdin = child.stdin.take().unwrap();
            io::copy(&mut input, &mut stdin)?;
            child.wait()?;
            th.join().unwrap();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn test_gen() {
        let mut gen = Gen::new();
        let v1 = gen.gen_string();
        let v2 = gen.gen_string();
        let v3 = gen.gen_string();
        assert_ne!(v1, v2);
        assert_ne!(v2, v3);
    }

    #[test]
    fn test_prep_process() {
        let mut gen = Gen::new();
        let plugin = Plugin {
            name: "foo".into(),
            path: "bar".into(),
            args: Some(vec!["--baz".into(), "$INPUT".into()]),
            stdin: None,
            stdout: Some(true),
        };
        let proc = prep_process(&mut gen, &plugin);
        assert_eq!(
            proc.input_file_name,
            proc.command
                .get_args()
                .nth(1)
                .and_then(|x| x.to_str())
                .map(String::from)
        );
        assert!(proc.output_file_name.is_none());
    }

    #[test]
    fn test_run_process() {
        let mut gen = Gen::new();
        let plugin = Plugin {
            name: "foo".into(),
            path: "/bin/sh".into(),
            args: Some(vec!["$INPUT".into()]),
            stdin: None,
            stdout: Some(true),
        };
        let proc = prep_process(&mut gen, &plugin);
        let input_file_name = proc.input_file_name.clone().unwrap();
        let input = Cursor::new(b"echo $INPUT");
        run_process(proc, input, File::create("test_run_process").unwrap()).unwrap();
        let mut output = File::open("test_run_process").unwrap();
        let mut result = [0u8; 16];
        output.read_exact(&mut result).unwrap();
        fs::remove_file("test_run_process").unwrap();
        assert_eq!(String::from_utf8_lossy(&result), input_file_name);
    }

    #[test]
    fn test_process_files() {
        let plugin = Plugin {
            name: "foo".into(),
            path: "/bin/sh".into(),
            args: Some(vec!["$INPUT".into()]),
            stdin: None,
            stdout: Some(true),
        };
        let mut config = HashMap::new();
        config.insert(String::from("script.sh"), plugin);
        let files = vec![Ok(NamedCursor(
            "bar".into(),
            Cursor::new((*b"#!/bin/sh\necho foobar").into()),
        ))];
        let open_output = |_| File::create("test_process_files");
        process_files(&config, files.into_iter(), open_output).unwrap();
        let mut output = File::open("test_process_files").unwrap();
        let mut result = [0u8; 6];
        output.read_exact(&mut result).unwrap();
        fs::remove_file("test_process_files").unwrap();
        assert_eq!(result, *b"foobar");
    }

    struct NamedCursor(String, Cursor<Vec<u8>>);

    impl Name for NamedCursor {
        fn name(&self) -> io::Result<String> {
            Ok(self.0.clone())
        }
    }

    impl Read for NamedCursor {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.1.read(buf)
        }
    }
}
