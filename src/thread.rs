use std::io::Write;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crossbeam_channel::{unbounded, Receiver, RecvError, Sender};
use log::{debug, error};

use crate::input::{Input, InputFactory};
use crate::output::Output;
use crate::plugin::Config;
use crate::pre_process::PreProcessor;

pub struct Pool<E> {
    pub factory: Arc<InputFactory>,
    pub input_sender: Sender<Input>,
    input_receiver: Receiver<Input>,
    output_sender: Sender<Output>,
    output_receiver: Receiver<Output>,
    activity_sender: Sender<bool>,
    activity_receiver: Receiver<bool>,
    active_threads: usize,
    pre_processor: Arc<PreProcessor>,
    exit: E,
}

impl<E: Write + Clone + Send + 'static> Pool<E> {
    pub fn new(config: Config, exit: E) -> Pool<E> {
        let (input_sender, input_receiver) = unbounded();
        let (output_sender, output_receiver) = unbounded();
        let (activity_sender, activity_receiver) = unbounded();
        Pool {
            factory: Arc::new(InputFactory::new()),
            pre_processor: Arc::new(PreProcessor::new(&config)),
            active_threads: 0,
            input_sender,
            input_receiver,
            output_sender,
            output_receiver,
            activity_sender,
            activity_receiver,
            exit,
        }
    }
    pub fn add_input_threads(&self, num: usize) {
        for _ in 0..num {
            let handler = InputHandler {
                factory: self.factory.clone(),
                input_receiver: self.input_receiver.clone(),
                input_sender: self.input_sender.clone(),
                output_sender: self.output_sender.clone(),
                activity_sender: self.activity_sender.clone(),
                pre_processor: self.pre_processor.clone(),
            };
            thread::spawn(move || handler.run());
        }
    }

    pub fn add_output_threads(&self, num: usize) {
        for _ in 0..num {
            let mut exit = self.exit.clone();
            let receiver = self.output_receiver.clone();
            let sender = self.activity_sender.clone();
            thread::spawn(move || run_thread(&receiver, &sender, |o| handle_output(&mut exit, o)));
        }
    }

    pub fn join(&mut self) -> Result<(), RecvError> {
        thread::sleep(Duration::from_millis(10));
        while self.active_threads > 0
            || !self.input_receiver.is_empty()
            || !self.output_receiver.is_empty()
            || !self.activity_receiver.is_empty()
        {
            self.active_threads = if self.activity_receiver.recv()? {
                self.active_threads + 1
            } else {
                self.active_threads - 1
            };
            thread::sleep(Duration::from_millis(10));
        }
        debug!("Thread pool joined");
        Ok(())
    }
}

fn run_thread<T, F: FnMut(T)>(receiver: &Receiver<T>, activity_sender: &Sender<bool>, mut f: F) {
    loop {
        let msg = receiver.recv().unwrap();
        activity_sender.send(true).unwrap();
        f(msg);
        activity_sender.send(false).unwrap();
    }
}

#[derive(Clone)]
struct InputHandler {
    factory: Arc<InputFactory>,
    pre_processor: Arc<PreProcessor>,
    input_receiver: Receiver<Input>,
    input_sender: Sender<Input>,
    output_sender: Sender<Output>,
    activity_sender: Sender<bool>,
}

impl InputHandler {
    fn handle_input(&self, input: Input) {
        let task_id = input.task_id;
        let path = input.item_path.clone();
        debug!(
            "{}: START Input {:?} data: {:?}",
            input.task_id, path, input.data
        );
        if let Some(err) = input
            .handle(
                &self.factory,
                &self.pre_processor,
                |x| self.schedule_input(x),
                |x| self.output_sender.send(x).unwrap(),
            )
            .err()
        {
            error!("{}: FINISH Input {:?} error: {:?}", task_id, path, err)
        } else {
            debug!("{}: FINISH Input {:?}", task_id, path);
        }
    }

    fn schedule_input(&self, input: Input) {
        if input.data.is_stdout() {
	    let clone = self.clone();
            thread::spawn(move || {
                clone.handle_input(input);
            });
        } else {
            self.input_sender.send(input).unwrap();
        }
    }

    fn run(self) {
        run_thread(&self.input_receiver, &self.activity_sender, |x| {
            self.handle_input(x)
        })
    }
}

fn handle_output<E: Write>(exit: &mut E, output: Output) {
    let task_id = output.task_id;
    let path = output.item_path.clone();
    let plugin = output.plugin_name.clone();
    debug!(
        "{}: START Output {:?} data: {:?}",
        output.task_id, path, output.data
    );
    if let Some(err) = output.handle(exit).err() {
        error!(
            "{}: FINISH Output {:?} plugin: {}, error: {:?}",
            task_id, path, plugin, err
        )
    } else {
        debug!("{}: FINISH Output {:?} plugin: {}", task_id, path, plugin);
    }
}
