use log::{debug, error, info, warn};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Cursor, Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use threadpool::ThreadPool;
use walkdir::WalkDir;
use yara::{Metadata, MetadataValue, Rules};

use crate::plugin::{self, Config, FileType, InputFile, InputType, OutputType, PreppedProcess};

pub trait Name {
    fn name(&self) -> io::Result<String>;
}

#[derive(Clone)]
pub struct Global {
    primary_pool: ThreadPool,
    secondary_pool: ThreadPool,
    conf: Arc<Config>,
    rules: Arc<Rules>,
}

impl Global {
    pub fn new(conf: Config, rules: Rules, primary_size: usize, secondary_size: usize) -> Global {
        Global {
            primary_pool: ThreadPool::new(primary_size),
            secondary_pool: ThreadPool::new(secondary_size),
            conf: Arc::new(conf),
            rules: Arc::new(rules),
        }
    }

    fn execute_primary<T>(&self, f: T)
    where
        T: FnOnce() -> io::Result<()> + Send + 'static,
    {
        spawn(&self.primary_pool, f)
    }

    fn execute_secondary<T>(&self, f: T)
    where
        T: FnOnce() -> io::Result<()> + Send + 'static,
    {
        spawn(&self.secondary_pool, f)
    }

    pub fn join(&self) {
        while self.primary_pool.active_count()
            + self.secondary_pool.active_count()
            + self.primary_pool.queued_count()
            + self.secondary_pool.queued_count()
            > 0
        {
            self.primary_pool.join();
            self.secondary_pool.join();
        }
    }
}

fn spawn<T>(tp: &ThreadPool, f: T)
where
    T: FnOnce() -> io::Result<()> + Send + 'static,
{
    tp.execute(|| match f() {
        Ok(()) => {}
        Err(x) => {
            error!("{:?}", x);
        }
    })
}

pub struct Input<T> {
    pub name: String,
    pub data: T,
}

impl<T> Input<T> {
    pub fn new<U: Into<String>>(name: U, data: T) -> Input<T> {
        Input {
            name: name.into(),
            data,
        }
    }
}

impl<T: Read> Read for Input<T> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.data.read(buf)
    }

    fn read_exact(&mut self, buf: &mut [u8]) -> io::Result<()> {
        self.data.read_exact(buf)
    }
}

pub fn process_dir<T: Write + Clone + Send + 'static>(
    global: Global,
    path: PathBuf,
    cleanup: bool,
    output: T,
) -> io::Result<()> {
    for entry in WalkDir::new(&path).into_iter().map(|x| x.unwrap()) {
        let file_path = entry.path().to_owned();
        if file_path.is_file() {
            let glob = global.clone();
            let outp = output.clone();
            global.execute_primary(move || process_file(glob, file_path, cleanup, outp));
        }
    }
    Ok(())
}

pub fn process_file<T: Write + Clone + Send + 'static>(
    global: Global,
    path: PathBuf,
    cleanup: bool,
    output: T,
) -> io::Result<()> {
    debug!("Started processing input file: {:?}", path);
    let input_file = File::open(&path)?;
    let name = path.file_name().unwrap().to_str().unwrap().to_string();
    process_input(
        global,
        Input::new(name, input_file),
        Some(InputFile::new(path.clone(), cleanup)),
        output,
    )?;
    debug!("Finished processing input file: {:?}", path);
    Ok(())
}

pub fn process_input<T: Read, U: Write + Clone + Send + 'static>(
    global: Global,
    mut input: Input<T>,
    input_file: Option<InputFile>,
    output: U,
) -> io::Result<()> {
    let mut buf = Vec::with_capacity(4096);
    buf.clear();
    (&mut input).take(4096).read_to_end(&mut buf)?;
    match get_file_type(&global.rules, &buf) {
        Some(f) => match global.conf.get(&f) {
            Some(p) => {
                let cur = Cursor::new(&mut buf);
                let input = Input::new(input.name, cur.chain(input.data));
                let proc = plugin::prep_process(p, input_file);
                debug!("Prepped process: {:?}", proc);
                info!("Processing {} (type: {}) with {}", input.name, f, p.name);
                run_process(global, proc, input, output)?;
            }
            None => {
                if let Some(infile) = input_file {
                    infile.cleanup()?;
                }
                warn!("File type for {} not included in config: {}", input.name, f)
            }
        },
        None => {
            if let Some(infile) = input_file {
                infile.cleanup()?;
            }
            warn!("File type for {} was not determined", input.name)
        }
    }
    Ok(())
}

fn run_process<T: Read, U: Write + Clone + Send + 'static>(
    global: Global,
    mut proc: PreppedProcess,
    mut input: Input<T>,
    output: U,
) -> io::Result<()> {
    proc.prepare_input(&mut input)?;
    let mut child = proc.command.spawn()?;
    let stderr = child.stderr.take().unwrap();
    let plugin_name = proc.plugin_name.clone();
    global.execute_secondary(|| handle_stderr(stderr, plugin_name));
    if proc.output_type == OutputType::stdout {
        let stdout = child.stdout.take().unwrap();
        let unpacker = proc.unpacker;
        let plugin_name = proc.plugin_name.clone();
        let input_name = input.name.clone();
        let glob = global.clone();
        let outp = output.clone();
        global.execute_secondary(move || {
            handle_stdout(glob, unpacker, plugin_name, input_name, stdout, outp)
        });
    }
    if proc.input_type == InputType::stdin {
        let mut stdin = child.stdin.take().unwrap();
        io::copy(&mut input, &mut stdin)?;
    }
    child.wait()?;
    proc.cleanup_input()?;
    if proc.output_type != OutputType::stdout {
        let glob = global.clone();
        global.execute_secondary(move || handle_output_files(glob, proc, output));
    }
    Ok(())
}

fn handle_stdout<T: Read, U: Write + Clone + Send + 'static>(
    global: Global,
    unpacker: bool,
    plugin_name: String,
    input_name: String,
    stdout: T,
    mut output: U,
) -> io::Result<()>
where
{
    if unpacker {
        process_input(global, Input::new(input_name, stdout), None, output)
    } else {
        copy_output(&plugin_name, stdout, &mut output)
    }
}

fn handle_output_files<T: Write + Clone + Send + 'static>(
    global: Global,
    proc: PreppedProcess,
    mut output: T,
) -> io::Result<()> {
    if proc.output_type == OutputType::file {
        if proc.unpacker {
            let path = proc.output_path.unwrap();
            process_file(global, path, true, output)?
        } else {
            let path = proc.output_path.unwrap();
            let output_file = File::open(&path)?;
            copy_output(&proc.plugin_name, output_file, &mut output)?;
            fs::remove_file(path)?
        }
    } else if proc.output_type == OutputType::dir {
        if proc.unpacker {
            process_dir(global, proc.output_path.unwrap(), true, output)?;
        } else {
            let path = proc.output_path.unwrap();
            for entry in WalkDir::new(&path).into_iter().map(|x| x.unwrap()) {
                let file_path = entry.path();
                if file_path.is_file() {
                    let output_file = File::open(&file_path)?;
                    copy_output(&proc.plugin_name, output_file, &mut output)?;
                    fs::remove_file(file_path)?;
                }
            }
            fs::remove_dir_all(&path)?
        }
    }
    Ok(())
}

fn handle_stderr<T: Read>(rdr: T, plugin_name: String) -> io::Result<()> {
    let mut bufrdr = BufReader::new(rdr);
    let mut buf = String::new();
    while bufrdr.read_line(&mut buf)? > 0 {
        error!("PLUGIN {}: {}", plugin_name, buf.trim());
        buf.clear();
    }
    Ok(())
}

static NEWLINE: u8 = b"\n"[0];

static COLON: u8 = b":"[0];

fn copy_output<T: Read, U: Write>(
    plugin_name: &String,
    child_output: T,
    output: &mut U,
) -> io::Result<()> {
    let mut rdr = BufReader::with_capacity(1024 * 1024, child_output);
    let mut buf = Vec::new();
    buf.extend_from_slice(plugin_name.as_bytes());
    buf.push(COLON);
    while rdr.read_until(NEWLINE, &mut buf)? > 0 {
        buf.pop();
        if buf.ends_with(b"\r") {
            buf.pop();
        }
        buf.push(NEWLINE);
        output.write_all(&buf)?;
        buf.truncate(plugin_name.len() + 1);
    }
    Ok(())
}

pub fn get_file_type(rules: &Rules, head: &[u8]) -> Option<FileType> {
    rules
        .scan_mem(head, u16::MAX)
        .unwrap()
        .iter()
        .next()
        .map(|x| {
            x.metadatas
                .iter()
                .filter(|x| x.identifier == "type")
                .next()
                .and_then(meta_string)
                .unwrap()
        })
}

fn meta_string(meta: &Metadata) -> Option<String> {
    match meta.value {
        MetadataValue::String(x) => Some(x.into()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::Plugin;
    use std::{collections::HashMap, sync::Mutex, thread, time::Duration};
    use yara::Compiler;

    #[test]
    fn test_run_process() {
        //env_logger::builder().is_test(true).try_init().unwrap();
        let global = Global::new(HashMap::new(), sh_rules(), 2, 4);
        let plugin = Plugin {
            name: "foo".into(),
            path: "/bin/sh".into(),
            args: Some(vec!["$INPUT".into()]),
            input: Some(InputType::file),
            output: Some(OutputType::stdout),
            unpacker: None,
        };
        let input = Input::new("", Cursor::new(b"echo $INPUT"));
        let proc = plugin::prep_process(&plugin, None);
        let mut expected = Vec::new();
        expected.extend_from_slice(b"foo:");
        expected.extend_from_slice(
            &proc
                .input_path()
                .as_ref()
                .unwrap()
                .to_str()
                .unwrap()
                .as_bytes(),
        );
        expected.push(NEWLINE);
        let output = SharedCursor::new();
        run_process(global.clone(), proc, input, output.clone()).unwrap();
        global.join();
        assert_eq!(output.into_inner(), expected);
    }

    #[test]
    fn test_process_input() {
        let plugin = Plugin {
            name: "foo".into(),
            path: "/bin/sh".into(),
            args: Some(vec!["$INPUT".into()]),
            input: None,
            output: Some(OutputType::stdout),
            unpacker: None,
        };
        let mut config = HashMap::new();
        config.insert(String::from("script/sh"), plugin);
        let file = Input::new("bar", Cursor::new(Vec::from(*b"#!/bin/sh\necho foobar")));
        let global = Global::new(config, sh_rules(), 2, 4);
        let output = SharedCursor::new();
        process_input(global.clone(), file, None, output.clone()).unwrap();
        assert_eq!(output.into_inner(), b"foo:foobar\n");
    }

    #[derive(Clone)]
    struct SharedCursor(Arc<Mutex<Cursor<Vec<u8>>>>);

    impl SharedCursor {
        fn new() -> SharedCursor {
            SharedCursor(Arc::new(Mutex::new(Cursor::new(Vec::new()))))
        }

        fn into_inner(self) -> Vec<u8> {
            while Arc::strong_count(&self.0) > 1 {
                thread::sleep(Duration::from_millis(10))
            }
            Arc::try_unwrap(self.0)
                .unwrap()
                .into_inner()
                .unwrap()
                .into_inner()
        }
    }

    impl Write for SharedCursor {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().write(buf)
        }

        fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
            self.0.lock().unwrap().write_all(buf)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.0.lock().unwrap().flush()
        }
    }

    fn sh_rules() -> Rules {
        let rules = r#"
rule ScriptSh
{
    meta: type = "script/sh"
    strings: $sb = { 23 21 }
    condition: $sb at 0
}
"#;
        let mut compiler = Compiler::new().unwrap();
        compiler.add_rules_str(rules).unwrap();
        compiler.compile_rules().unwrap()
    }
}
