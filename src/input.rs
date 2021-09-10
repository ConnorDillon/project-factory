use std::fs::{self, File};
use std::io::{self, BufReader, Read, Stdin};
use std::path::PathBuf;
use std::process::ChildStdout;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use log::debug;

use crate::output::{Output, OutputData, TaskId, BUFSIZE};
use crate::plugin::OutputPath;
use crate::pre_process::{PreProcessedInput, PreProcessor};
use crate::walk;

pub struct InputFactory {
    pub last_id: AtomicU64,
}

impl InputFactory {
    pub fn new() -> InputFactory {
        InputFactory {
            last_id: AtomicU64::new(0),
        }
    }

    pub fn new_input<P: Into<PathBuf>>(&self, item_path: P, data: InputData) -> Input {
        Input {
            task_id: TaskId::new(self.last_id.fetch_add(1, Ordering::Relaxed)),
            item_path: item_path.into(),
            data,
        }
    }
}

#[derive(Debug)]
pub struct Input {
    pub task_id: TaskId,
    pub item_path: PathBuf,
    pub data: InputData,
}

impl Input {
    pub fn handle<I: Fn(Input), O: Fn(Output)>(
        self,
        factory: Arc<InputFactory>,
        pre_processor: Arc<PreProcessor>,
        input_cb: &I,
        output_cb: &O,
    ) -> io::Result<()> {
        match self.data {
            InputData::File(path, temp) => {
                let file_buf = BufReader::with_capacity(BUFSIZE, File::open(&path)?);
                if let Some(ppi) = pre_processor.pre_process(
                    self.task_id,
                    self.item_path,
                    Some(&path),
                    file_buf,
                )? {
                    run_task(input_cb, output_cb, factory, ppi)?;
                }
                if temp {
                    fs::remove_file(path)?;
                }
            }
            InputData::Stdin(stdin) => {
                if let Some(ppi) =
                    pre_processor.pre_process(self.task_id, self.item_path, None, stdin)?
                {
                    run_task(input_cb, output_cb, factory, ppi)?;
                }
            }
            InputData::Stdout(stdout) => {
                if let Some(ppi) =
                    pre_processor.pre_process(self.task_id, self.item_path, None, stdout)?
                {
                    run_task(input_cb, output_cb, factory, ppi)?;
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
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

fn run_task<I, O, R>(
    input_cb: &I,
    output_cb: &O,
    factory: Arc<InputFactory>,
    mut ppi: PreProcessedInput<R>,
) -> io::Result<()>
where
    I: Fn(Input),
    O: Fn(Output),
    R: Read,
{
    let input_exists = ppi
        .plugin
        .input_path
        .file()
        .map(|x| x.exists())
        .unwrap_or(true);
    if !input_exists {
        let path = ppi.plugin.input_path.file().unwrap();
        debug!("{}: Creating input file {:?}", ppi.task_id, path);
        let mut file = File::create(path)?;
        io::copy(&mut ppi.data, &mut file)?;
    }
    if let Some(path) = ppi.plugin.output_path.dir() {
        debug!("{}: Creating dir {:?}", ppi.task_id, path);
        fs::create_dir(path)?;
    }

    let mut child = ppi.plugin.command.spawn()?;
    output_cb(Output::new(
        ppi.task_id,
        ppi.item_path.clone(),
        ppi.item_type.clone(),
        ppi.plugin.plugin_name.clone(),
        OutputData::LogStderr(child.stderr.take().unwrap()),
    ));
    let stdout = child.stdout.take().unwrap();
    if ppi.plugin.output_path.stdout() {
        if ppi.plugin.unpacker {
            input_cb(factory.new_input(ppi.item_path.clone(), InputData::Stdout(stdout)));
        } else {
            output_cb(Output::new(
                ppi.task_id,
                ppi.item_path.clone(),
                ppi.item_type.clone(),
                ppi.plugin.plugin_name.clone(),
                OutputData::Stdout(stdout),
            ));
        }
    } else {
        output_cb(Output::new(
            ppi.task_id,
            ppi.item_path.clone(),
            ppi.item_type.clone(),
            ppi.plugin.plugin_name.clone(),
            OutputData::LogStdout(stdout),
        ));
    }
    if ppi.plugin.input_path.stdin() {
        debug!("{}: Copy task data to child stdin", ppi.task_id);
        io::copy(&mut ppi.data, child.stdin.as_mut().unwrap())?;
    }
    child.wait()?;
    debug!("{}: FINISH CHILD PROCESS", ppi.task_id);

    if !input_exists {
        fs::remove_file(ppi.plugin.input_path.file().unwrap())?;
    }
    match ppi.plugin.output_path {
        OutputPath::Dir(path) => {
            if ppi.plugin.unpacker {
                walk::walk_dir(path, ppi.item_path, |p, ip| {
                    input_cb(factory.new_input(ip, InputData::File(p, true)));
                })?
            } else {
                let task_id = ppi.task_id;
                let plugin_name = ppi.plugin.plugin_name;
                let item_type = ppi.item_type;
                let item_path = ppi.item_path;
                walk::walk_dir(path, item_path.clone(), |p, _| {
                    output_cb(Output::new(
                        task_id,
                        item_path.clone(),
                        item_type.clone(),
                        plugin_name.clone(),
                        OutputData::File(p),
                    ));
                })?
            }
        }
        OutputPath::File(path) => {
            if ppi.plugin.unpacker {
                input_cb(factory.new_input(ppi.item_path, InputData::File(path, true)));
            } else {
                let output = Output::new(
                    ppi.task_id,
                    ppi.item_path,
                    ppi.item_type,
                    ppi.plugin.plugin_name,
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
    use serde_json::Value;

    use super::*;

    use std::io::{Cursor, Write};
    use std::sync::Mutex;
    use std::thread;
    use std::time::Duration;

    use crate::plugin::{OutputType, Plugin};

    #[test]
    fn test_run_task() {
        let factory = Arc::new(InputFactory::new());
        let plugin = Plugin {
            name: "foo".into(),
            path: "/bin/sh".into(),
            args: Some(vec!["$INPUT".into()]),
            input: None,
            output: Some(OutputType::stdout),
            unpacker: None,
        };
        let task = PreProcessedInput {
            task_id: TaskId::new(0),
            item_path: "".into(),
            item_type: "".into(),
            plugin: plugin.prep(None).unwrap(),
            data: Cursor::new(Vec::from(*b"#!/bin/sh\necho foobar")),
        };
        let cur = SharedCursor::new();
        let cur_clone = cur.clone();
        run_task(
            &drop,
            &move |x| x.handle(&mut cur_clone.clone()).unwrap(),
            factory,
            task,
        )
        .unwrap();
        let result: Value = serde_json::from_slice(&cur.into_inner()).unwrap();
        assert_eq!(
            result.as_object().unwrap().get("data").unwrap(),
            &Value::String("foobar".into())
        );
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
