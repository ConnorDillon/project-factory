use std::fs::{self, File};
use std::io::{self, BufReader, Read, Stdin};
use std::path::PathBuf;
use std::process::ChildStdout;
use std::sync::Arc;

use crate::output::{Output, OutputData, BUFSIZE};
use crate::plugin::OutputPath;
use crate::task::{Task, TaskFactory};
use crate::walk;

pub struct Input {
    pub item_path: PathBuf,
    pub data: InputData,
}

impl Input {
    pub fn new<P: Into<PathBuf>>(item_path: P, data: InputData) -> Input {
        Input {
            item_path: item_path.into(),
            data,
        }
    }

    pub fn handle<I: Fn(Input), O: Fn(Output)>(
        self,
        factory: Arc<TaskFactory>,
        input_cb: &I,
        output_cb: &O,
    ) -> io::Result<()> {
        match self.data {
            InputData::File(path, temp) => {
                let file_buf = BufReader::with_capacity(BUFSIZE, File::open(&path)?);
                if let Some(task) = factory.new_task(self.item_path, Some(&path), file_buf)? {
                    run_task(input_cb, output_cb, task)?;
                }
                if temp {
                    fs::remove_file(path)?;
                }
            }
            InputData::Stdin(stdin) => {
                if let Some(task) = factory.new_task(self.item_path, None, stdin)? {
                    run_task(input_cb, output_cb, task)?;
                }
            }
            InputData::Stdout(stdout) => {
                if let Some(task) = factory.new_task(self.item_path, None, stdout)? {
                    run_task(input_cb, output_cb, task)?;
                }
            }
        }
        Ok(())
    }
}

pub enum InputData {
    File(PathBuf, bool),
    Stdin(Stdin),
    Stdout(ChildStdout),
}

impl InputData {
    pub fn is_stdout(&self) -> bool {
        match self {
            &InputData::Stdout(_) => true,
            _ => false,
        }
    }
}

fn run_task<I, O, R>(input_cb: &I, output_cb: &O, mut task: Task<R>) -> io::Result<()>
where
    I: Fn(Input),
    O: Fn(Output),
    R: Read,
{
    let input_exists = task
        .plugin
        .input_path
        .file()
        .map(|x| x.exists())
        .unwrap_or(true);
    if !input_exists {
        let mut file = File::create(task.plugin.input_path.file().unwrap())?;
        io::copy(&mut task.data, &mut file)?;
    }
    if let Some(path) = task.plugin.output_path.dir() {
        fs::create_dir(path)?;
    }

    let mut child = task.plugin.command.spawn()?;
    output_cb(Output::new(
        task.item_path.clone(),
        task.plugin.plugin_name.clone(),
        OutputData::Stderr(child.stderr.take().unwrap()),
    ));
    if task.plugin.output_path.stdout() {
        if task.plugin.unpacker {
            input_cb(Input::new(
                task.item_path.clone(),
                InputData::Stdout(child.stdout.take().unwrap()),
            ));
        } else {
            output_cb(Output::new(
                task.item_path.clone(),
                task.plugin.plugin_name.clone(),
                OutputData::Stdout(child.stdout.take().unwrap()),
            ));
        }
    }
    if task.plugin.input_path.stdin() {
        io::copy(&mut task.data, child.stdin.as_mut().unwrap())?;
    }
    child.wait()?;

    if !input_exists {
        fs::remove_file(task.plugin.input_path.file().unwrap())?;
    }
    match task.plugin.output_path {
        OutputPath::Dir(path) => {
            if task.plugin.unpacker {
                walk::walk_dir(path, task.item_path, |p, ip| {
                    input_cb(Input::new(ip, InputData::File(p, true)));
                })?
            } else {
                let plugin_name = task.plugin.plugin_name;
                walk::walk_dir(path, task.item_path, |p, ip| {
                    output_cb(Output::new(ip, plugin_name.clone(), OutputData::File(p)));
                })?
            }
        }
        OutputPath::File(path) => {
            if task.plugin.unpacker {
                input_cb(Input::new(task.item_path, InputData::File(path, true)));
            } else {
                let output = Output::new(
                    task.item_path,
                    task.plugin.plugin_name,
                    OutputData::File(path),
                );
                output_cb(output);
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::{Cursor, Write};
    use std::sync::Mutex;
    use std::thread;
    use std::time::Duration;

    use crate::plugin::{OutputType, Plugin};

    #[test]
    fn test_run_task() {
        let plugin = Plugin {
            name: "foo".into(),
            path: "/bin/sh".into(),
            args: Some(vec!["$INPUT".into()]),
            input: None,
            output: Some(OutputType::stdout),
            unpacker: None,
        };
        let task = Task {
            item_path: "".into(),
            plugin: plugin.prep(None).unwrap(),
            data: Cursor::new(Vec::from(*b"#!/bin/sh\necho foobar")),
        };
        let cur = SharedCursor::new();
        let cur_clone = cur.clone();
        run_task(
            &drop,
            &move |x| x.handle(&mut cur_clone.clone()).unwrap(),
            task,
        )
        .unwrap();
        assert_eq!(cur.into_inner(), b"foo:foobar\n");
    }

    #[derive(Clone)]
    struct SharedCursor(Arc<Mutex<Cursor<Vec<u8>>>>);

    impl SharedCursor {
        fn new() -> SharedCursor {
            SharedCursor(Arc::new(Mutex::new(Cursor::new(Vec::new()))))
        }

        fn into_inner(self) -> Vec<u8> {
            if Arc::strong_count(&self.0) > 1 {
                thread::sleep(Duration::from_millis(100))
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
}
