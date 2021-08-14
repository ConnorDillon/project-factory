use log::{debug, error, info, trace, warn, Level};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, BufWriter, Cursor, Read, Write};
use std::process::Child;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use tar::Entry;
use threadpool::ThreadPool;
use yara::{Metadata, MetadataValue, Rules};

use crate::plugin::{self, Config, FileType, Gen, InputType, OutputType, PreppedProcess};

pub fn process_files<
    T: Iterator<Item = io::Result<U>>,
    U: Read + Name,
    V: Fn(String) -> io::Result<W>,
    W: Write + Send + 'static,
>(
    conf: &Config,
    iter: T,
    output: V,
    rules: Rules,
) -> io::Result<()> {
    let tp = ThreadPool::new(num_cpus::get() * 6);
    let pc = ProcessCount::new(num_cpus::get() * 2);
    let mut gen = Gen::new();
    let mut buf = Vec::with_capacity(4096);
    for entry_result in iter {
        let mut entry = entry_result?;
        let name = entry.name()?;
        buf.clear();
        (&mut entry).take(4096).read_to_end(&mut buf)?;
        match get_file_type(&rules, &buf) {
            Some(f) => match conf.get(&f) {
                Some(p) => {
                    let cur = Cursor::new(&mut buf);
                    let proc = plugin::prep_process(&mut gen, p);
                    info!("Processing {} (type: {}) with {}", name, f, p.name);
                    run_process(&tp, pc.clone(), proc, cur.chain(entry), output(f)?)?;
                }
                None => warn!("File type for {} not included in config: {}", name, f),
            },
            None => warn!("File type for {} was not determined", name),
        }
    }
    tp.join();
    Ok(())
}

#[derive(Clone)]
struct ProcessCount(Arc<(Mutex<usize>, Condvar, usize)>);

impl ProcessCount {
    fn new(max: usize) -> ProcessCount {
        ProcessCount(Arc::new((Mutex::new(0), Condvar::new(), max)))
    }

    fn wait(&self, count: usize) -> MutexGuard<usize> {
        let (lock, cvar, _) = &*self.0;
        cvar.wait_while(lock.lock().unwrap(), |c| *c > count)
            .unwrap()
    }

    fn incr(&self) {
        let mut count = self.wait(self.0 .2);
        *count = *count + 1;
    }

    fn decr(&self) {
        let mut count = self.0 .0.lock().unwrap();
        *count = *count - 1;
        self.0 .1.notify_all();
    }
}

fn get_file_type(rules: &Rules, head: &[u8]) -> Option<FileType> {
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

pub trait Name {
    fn name(&self) -> io::Result<String>;
}

impl<T: Read> Name for Entry<'_, T> {
    fn name(&self) -> io::Result<String> {
        let path = self.path()?;
        Ok(String::from(path.file_name().unwrap().to_str().unwrap()))
    }
}

fn run_process<T: Read, U: Write + Send + 'static>(
    tp: &ThreadPool,
    pc: ProcessCount,
    mut proc: PreppedProcess,
    mut input: T,
    output: U,
) -> io::Result<()> {
    if proc.output_type == OutputType::dir {
        fs::create_dir(proc.output_file_name.as_ref().unwrap())?;
    }
    match proc.input_type {
        InputType::file => {
            let input_path = proc.input_file_name.as_ref().unwrap();
            io::copy(&mut input, &mut File::create(&input_path)?)?;
            pc.incr();
            let child = proc.command.spawn()?;
            spawn_output_handlers(tp, pc, proc, output, child);
        }
        InputType::stdin => {
            pc.incr();
            let mut child = proc.command.spawn()?;
            let mut stdin = child.stdin.take().unwrap();
            spawn_output_handlers(tp, pc, proc, output, child);
            io::copy(&mut input, &mut stdin)?;
        }
    };
    Ok(())
}

fn spawn_output_handlers<U: Write + Send + 'static>(
    tp: &ThreadPool,
    pc: ProcessCount,
    proc: PreppedProcess,
    mut output: U,
    mut child: Child,
) {
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    spawn_logger(tp, Level::Error, stderr, proc.plugin_name.clone());
    match proc.output_type {
        OutputType::file => {
            spawn_logger(tp, Level::Info, stdout, proc.plugin_name.clone());
            spawn(tp, move || {
                wait_and_cleanup(&pc, &mut child, &proc)?;
                if !proc.unpacker {
                    let output_path = proc.output_file_name.unwrap();
                    let output_file = File::open(&output_path)?;
                    copy_output(&proc.plugin_name, output_file, &mut output)?;
                    fs::remove_file(&output_path)?;
                }
                Ok(())
            });
        }
        OutputType::dir => {
            spawn_logger(tp, Level::Info, stdout, proc.plugin_name.clone());
            spawn(tp, move || {
                wait_and_cleanup(&pc, &mut child, &proc)?;
                if !proc.unpacker {
                    let output_path = proc.output_file_name.as_ref().unwrap();
                    for entry in fs::read_dir(&output_path)? {
                        let file_path = entry?.path();
                        let output_file = File::open(&file_path)?;
                        copy_output(&proc.plugin_name, output_file, &mut output)?;
                        fs::remove_file(file_path)?;
                    }
                    fs::remove_dir(&output_path)?;
                }
                Ok(())
            });
        }
        OutputType::stdout => spawn(tp, move || {
            if !proc.unpacker {
                copy_output(&proc.plugin_name, stdout.unwrap(), &mut output)?;
            }
            wait_and_cleanup(&pc, &mut child, &proc)
        }),
    }
}

fn wait_and_cleanup(pc: &ProcessCount, child: &mut Child, proc: &PreppedProcess) -> io::Result<()> {
    child.wait()?;
    pc.decr();
    if let Some(i) = proc.input_file_name.as_ref() {
        fs::remove_file(i)?;
    }
    Ok(())
}

fn spawn_logger<T>(tp: &ThreadPool, level: Level, rdr: Option<T>, plugin_name: String)
where
    T: Read + Send + 'static,
{
    let mut bufrdr = BufReader::new(rdr.unwrap());
    spawn(tp, move || {
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

fn spawn<F, T>(tp: &ThreadPool, f: F)
where
    F: FnOnce() -> io::Result<T>,
    F: Send + 'static,
    T: Send + 'static,
{
    tp.execute(|| match f() {
        Ok(_) => {}
        Err(x) => {
            error!("{:?}", x);
        }
    })
}

static NEWLINE: u8 = b"\n"[0];

static COLON: u8 = b":"[0];

fn copy_output<T: Read, U: Write>(
    plugin_name: &String,
    child_output: T,
    output: &mut U,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::Plugin;
    use std::collections::HashMap;
    use yara::Compiler;

    #[test]
    fn test_run_process() {
        //env_logger::builder().is_test(true).try_init().unwrap();
        let tp = ThreadPool::new(3);
        let pc = ProcessCount::new(1);
        let mut gen = Gen::new();
        let plugin = Plugin {
            name: "foo".into(),
            path: "/bin/sh".into(),
            args: Some(vec!["$INPUT".into()]),
            input: Some(InputType::file),
            output: Some(OutputType::stdout),
            unpacker: None,
        };
        let proc = plugin::prep_process(&mut gen, &plugin);
        let mut expected = Vec::new();
        expected.extend_from_slice(b"foo:");
        expected.extend_from_slice(&proc.input_file_name.as_ref().unwrap().as_bytes());
        expected.push(NEWLINE);
        let input = Cursor::new(b"echo $INPUT");
        run_process(
            &tp,
            pc,
            proc,
            input,
            File::create("test_run_process").unwrap(),
        )
        .unwrap();
        tp.join();
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
            input: None,
            output: Some(OutputType::stdout),
            unpacker: None,
        };
        let mut config = HashMap::new();
        config.insert(String::from("script/sh"), plugin);
        let files = vec![Ok(NamedCursor(
            "bar".into(),
            Cursor::new((*b"#!/bin/sh\necho foobar").into()),
        ))];
        let open_output = |_| File::create("test_process_files");
        process_files(&config, files.into_iter(), open_output, sh_rules()).unwrap();
        let mut output = File::open("test_process_files").unwrap();
        let mut result = Vec::new();
        output.read_to_end(&mut result).unwrap();
        fs::remove_file("test_process_files").unwrap();
        assert_eq!(&result, b"foo:foobar\n");
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
