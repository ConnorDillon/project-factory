#![feature(command_access)]

use getopts::Options;
use log::{debug, error, info, trace, warn, Level};
use serde::{Deserialize, Serialize};
use serde_yaml::from_reader;
use std::collections::hash_map::{DefaultHasher, RandomState};
use std::collections::HashMap;
use std::fs::{self, File};
use std::hash::{BuildHasher, Hash, Hasher};
use std::io::{self, BufRead, BufReader, BufWriter, Cursor, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
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
        debug!("Config: {:?}", conf);
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
    let tc = ThreadCount::new();
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
                    info!("Processing {} (type: {}) with {}", name, f, p.name);
                    run_process(&tc, proc, cur.chain(entry), output(f)?)?;
                }
                None => error!("File type not included in config: {}", f),
            },
            None => warn!("File type not determined for {}", name),
        }
    }
    tc.wait();
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

#[derive(Clone)]
struct ThreadCount(Arc<(Mutex<u32>, Condvar)>);

impl ThreadCount {
    fn new() -> ThreadCount {
        ThreadCount(Arc::new((Mutex::new(0u32), Condvar::new())))
    }

    fn wait(&self) {
        let (lock, cvar) = &*self.0;
        let _guard = cvar.wait_while(lock.lock().unwrap(), |c| *c > 0).unwrap();
    }

    fn incr(&self) {
        let mut count = self.0 .0.lock().unwrap();
        *count = *count + 1;
    }

    fn decr(&self) {
        let mut count = self.0 .0.lock().unwrap();
        *count = *count - 1;
        self.0 .1.notify_all();
    }
}

fn get_file_type(_name: &str, head: &[u8]) -> Option<FileType> {
    if head.len() >= 4 && &head[..4] == b"FILE" {
        Some("ntfs.mft".into())
    } else if head.len() >= 9 && &head[..9] == b"#!/bin/sh" {
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
    let cwd = env::current_dir().unwrap();
    let input_file_name = if plugin.stdin.unwrap_or(false) {
        cmd.stdin(Stdio::piped());
        None
    } else {
        cmd.stdin(Stdio::null());
        let mut input_path = cwd.clone();
        input_path.push(gen.gen_string());
        let input_path_str = String::from(input_path.to_str().unwrap());
        cmd.env("INPUT", &input_path);
        replace_arg(&mut args, "$INPUT", &input_path_str);
        Some(input_path_str)
    };
    let output_file_name = if plugin.stdout.unwrap_or(false) {
        None
    } else {
        let mut output_cwd = cwd.clone();
        let output_dir = String::from(output_cwd.to_str().unwrap());
        replace_arg(&mut args, "$OUTPUT_DIR", &output_dir);
        cmd.env("OUTPUT_DIR", &output_dir);
        let output_file = gen.gen_string();
        replace_arg(&mut args, "$OUTPUT_FILE", &output_file);
        cmd.env("OUTPUT_FILE", &output_file);
        output_cwd.push(output_file);
        let output = String::from(output_cwd.to_str().unwrap());
        cmd.env("OUTPUT", &output);
        replace_arg(&mut args, "$OUTPUT", &output);
        Some(output)
    };
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    PreppedProcess {
        command: cmd,
        input_file_name,
        output_file_name,
        plugin_name: plugin.name.clone(),
    }
}

struct PreppedProcess {
    command: Command,
    input_file_name: Option<String>,
    output_file_name: Option<String>,
    plugin_name: String,
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
    tc: &ThreadCount,
    mut proc: PreppedProcess,
    mut input: T,
    output: U,
) -> io::Result<()> {
    let plugin_name = proc.plugin_name;
    match (proc.input_file_name, proc.output_file_name) {
        (Some(i), Some(o)) => {
            io::copy(&mut input, &mut File::create(&i)?)?;
            let mut child = proc.command.spawn()?;
            spawn_logger(tc, Level::Info, child.stdout.take(), plugin_name.clone());
            spawn_logger(tc, Level::Error, child.stderr.take(), plugin_name.clone());
            spawn(tc.clone(), move || {
                child.wait()?;
                let output_file = File::open(&o)?;
                copy_output(plugin_name, output_file, output)?;
                fs::remove_file(i)?;
                fs::remove_file(o)
            });
        }
        (Some(i), None) => {
            io::copy(&mut input, &mut File::create(&i)?)?;
            let mut child = proc.command.spawn()?;
            spawn_logger(tc, Level::Error, child.stderr.take(), plugin_name.clone());
            spawn(tc.clone(), move || {
                let stdout = child.stdout.take().unwrap();
                copy_output(plugin_name, stdout, output)?;
                child.wait()?;
                fs::remove_file(i)
            });
        }
        (None, Some(o)) => {
            File::create(&o)?;
            let mut child = proc.command.spawn()?;
            let mut stdin = child.stdin.take().unwrap();
            spawn_logger(tc, Level::Info, child.stdout.take(), plugin_name.clone());
            spawn_logger(tc, Level::Error, child.stderr.take(), plugin_name.clone());
            io::copy(&mut input, &mut stdin)?;
            spawn(tc.clone(), move || {
                child.wait()?;
                let output_file = File::open(&o)?;
                copy_output(plugin_name, output_file, output)?;
                fs::remove_file(o)
            });
        }
        (None, None) => {
            let mut child = proc.command.spawn()?;
            spawn_logger(tc, Level::Error, child.stderr.take(), plugin_name.clone());
            let stdout = child.stdout.take().unwrap();
            spawn(tc.clone(), move || copy_output(plugin_name, stdout, output));
            let mut stdin = child.stdin.take().unwrap();
            io::copy(&mut input, &mut stdin)?;
        }
    }
    Ok(())
}

fn spawn_logger<T>(
    tc: &ThreadCount,
    level: Level,
    rdr: Option<T>,
    plugin_name: String,
) -> JoinHandle<()>
where
    T: Read + Send + 'static,
{
    let mut bufrdr = BufReader::new(rdr.unwrap());
    spawn(tc.clone(), move || {
        let mut buf = String::new();
        while bufrdr.read_line(&mut buf)? > 0 {
            match level {
                Level::Error => error!("PLUGIN {}: {}", plugin_name, buf.trim()),
                Level::Warn => warn!("PLUGIN {}: {}", plugin_name, buf.trim()),
                Level::Info => info!("PLUGIN {}: {}", plugin_name, buf.trim()),
                Level::Debug => debug!("PLUGIN {}: {}", plugin_name, buf.trim()),
                Level::Trace => trace!("PLUGIN {}: {}", plugin_name, buf.trim()),
            }
            buf.clear();
        }
        Ok(())
    })
}

fn spawn<F, T>(tc: ThreadCount, f: F) -> JoinHandle<T>
where
    F: FnOnce() -> io::Result<T>,
    F: Send + 'static,
    T: Send + 'static,
{
    tc.incr();
    thread::spawn(move || match f() {
        Ok(x) => {
            tc.decr();
            x
        }
        Err(x) => {
            error!("{:?}", x);
            tc.decr();
            drop(tc);
            Err(x).unwrap()
        }
    })
}

static NEWLINE: u8 = b"\n"[0];

static COLON: u8 = b":"[0];

fn copy_output<T: Read, U: Write>(
    plugin_name: String,
    child_output: T,
    output: U,
) -> io::Result<()> {
    let mut rdr = BufReader::with_capacity(1024 * 1024, child_output);
    let mut wrt = BufWriter::with_capacity(1024 * 1024, output);
    let mut buf = Vec::new();
    buf.extend_from_slice(plugin_name.as_bytes());
    buf.push(COLON);
    while rdr.read_until(NEWLINE, &mut buf)? > 0 {
        buf.pop();
        if buf.ends_with(b"\r") {
            buf.pop();
        }
        buf.push(NEWLINE);
        wrt.write(&buf)?;
        buf.truncate(plugin_name.len() + 1);
    }
    Ok(())
}

#[derive(Deserialize, Serialize, PartialEq, Debug)]
struct Line {
    plugin: String,
    output: String,
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

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
        //env_logger::builder().is_test(true).try_init().unwrap();
        let tc = ThreadCount::new();
        let mut gen = Gen::new();
        let plugin = Plugin {
            name: "foo".into(),
            path: "/bin/sh".into(),
            args: Some(vec!["$INPUT".into()]),
            stdin: None,
            stdout: Some(true),
        };
        let proc = prep_process(&mut gen, &plugin);
        let mut expected = Vec::new();
        expected.extend_from_slice(b"foo:");
        expected.extend_from_slice(&proc.input_file_name.as_ref().unwrap().as_bytes());
        expected.push(NEWLINE);
        let input = Cursor::new(b"echo $INPUT");
        run_process(&tc, proc, input, File::create("test_run_process").unwrap()).unwrap();
        tc.wait();
        let mut output = File::open("test_run_process").unwrap();
        let mut result = Vec::new();
        output.read_to_end(&mut result).unwrap();
        fs::remove_file("test_run_process").unwrap();
        assert_eq!(result, expected);
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
        thread::sleep(Duration::from_millis(100));
        let mut output = File::open("test_process_files").unwrap();
        let mut result = Vec::new();
        output.read_to_end(&mut result).unwrap();
        fs::remove_file("test_process_files").unwrap();
        assert_eq!(&result, b"foo:foobar\n");
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
