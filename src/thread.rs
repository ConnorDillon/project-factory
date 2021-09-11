use std::io::Write;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crossbeam_channel::{unbounded, Receiver, RecvError, Sender};
use log::{debug, error};
//use threadpool::ThreadPool;

use crate::input::{Input, InputFactory};
use crate::output::Output;
use crate::plugin::Config;
use crate::pre_process::PreProcessor;

pub struct Pool<E> {
    pub factory: Arc<InputFactory>,
    config: Arc<Config>,
    rules: Arc<String>,
    pub input_sender: Sender<Input>,
    input_receiver: Receiver<Input>,
    output_sender: Sender<Output>,
    output_receiver: Receiver<Output>,
    activity_sender: Sender<bool>,
    activity_receiver: Receiver<bool>,
    active_threads: usize,
    exit: E,
}

impl<E: Write + Clone + Send + 'static> Pool<E> {
    pub fn new(config: Config, rules: String, exit: E) -> Pool<E> {
        let (input_sender, input_receiver) = unbounded();
        let (output_sender, output_receiver) = unbounded();
        let (activity_sender, activity_receiver) = unbounded();
        Pool {
            factory: Arc::new(InputFactory::new()),
            active_threads: 0,
            config: Arc::new(config),
            rules: Arc::new(rules),
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
            let factory = self.factory.clone();
            let config = self.config.clone();
            let rules = self.rules.clone();
            let input_receiver = self.input_receiver.clone();
            let input_sender = self.input_sender.clone();
            let output_sender = self.output_sender.clone();
            let activity_sender = self.activity_sender.clone();
            thread::spawn(move || {
                let handler = InputHandler {
                    pre_processor: PreProcessor::new(config, rules),
                    factory,
                    input_receiver,
                    input_sender,
                    output_sender,
                    activity_sender,
                };
                handler.run()
            });
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

struct InputHandler {
    factory: Arc<InputFactory>,
    pre_processor: PreProcessor,
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
                self.factory.clone(),
                &self.pre_processor,
                &|x| self.schedule_input(x),
                &|x| self.output_sender.send(x).unwrap(),
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
            let factory = self.factory.clone();
            let config = self.pre_processor.config.clone();
            let rules = self.pre_processor.rules_str.clone();
            let input_receiver = self.input_receiver.clone();
            let input_sender = self.input_sender.clone();
            let output_sender = self.output_sender.clone();
            let activity_sender = self.activity_sender.clone();
            thread::spawn(move || {
                let handler = InputHandler {
                    pre_processor: PreProcessor::new(config, rules),
                    factory,
                    input_receiver,
                    input_sender,
                    output_sender,
                    activity_sender,
                };
                handler.handle_input(input);
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

// #[derive(Clone)]
// pub struct Pool<E> {
//     pub in_pool: ThreadPool,
//     pub out_pool: ThreadPool,
//     threads: Arc<Mutex<Vec<JoinHandle<()>>>>,
//     pub factory: Arc<InputFactory>,
//     pub pre_processor: Arc<PreProcessor>,
//     pub exit: E,
// }
//
// impl<E> Pool<E>
// where
//     E: Write + Clone + Send + 'static,
// {
//     pub fn new(
//         in_pool_size: usize,
//         out_pool_size: usize,
//         pre_processor: PreProcessor,
//         exit: E,
//     ) -> Pool<E> {
//         Pool {
//             in_pool: ThreadPool::new(in_pool_size),
//             out_pool: ThreadPool::new(out_pool_size),
//             factory: Arc::new(InputFactory::new()),
//             pre_processor: Arc::new(pre_processor),
//             threads: Arc::new(Mutex::new(Vec::new())),
//             exit,
//         }
//     }
//
//     pub fn process_input(&self, input: Input) {
//         let task_id = input.task_id;
//         let path = input.item_path.clone();
//         let is_stdout = input.data.is_stdout();
//         let clone = self.clone();
//         let job = move || {
//             debug!(
//                 "{}: START Input {:?} data: {:?}",
//                 input.task_id, path, input.data
//             );
//             let input_cb = |x| clone.process_input(x);
//             let output_cb = |x| clone.process_output(x);
//             if let Some(err) = input
//                 .handle(
//                     clone.factory.clone(),
//                     clone.pre_processor.clone(),
//                     &input_cb,
//                     &output_cb,
//                 )
//                 .err()
//             {
//                 error!("{}: FINISH Input {:?} error: {:?}", task_id, path, err)
//             } else {
//                 debug!("{}: FINISH Input {:?}", task_id, path);
//             }
//         };
//         if is_stdout {
//             let mut guard = self.threads.lock().unwrap();
//             guard.push(thread::spawn(job));
//         } else {
//             self.in_pool.execute(job);
//         }
//     }
//
//     pub fn process_output(&self, output: Output) {
//         let task_id = output.task_id;
//         let mut exit = self.exit.clone();
//         let path = output.item_path.clone();
//         let plugin = output.plugin_name.clone();
//         self.out_pool.execute(move || {
//             debug!(
//                 "{}: START Output {:?} data: {:?}",
//                 output.task_id, path, output.data
//             );
//             if let Some(err) = output.handle(&mut exit).err() {
//                 error!(
//                     "{}: FINISH Output {:?} plugin: {}, error: {:?}",
//                     task_id, path, plugin, err
//                 )
//             } else {
//                 debug!("{}: FINISH Output {:?} plugin: {}", task_id, path, plugin);
//             }
//         })
//     }
//
//     pub fn log_stats(&self) {
//         let mut oa = 1;
//         let mut iq = 1;
//         let mut ia = 1;
//         let mut oq = 1;
//         let mut ts = 1;
//         while iq + ia + oq + oa + ts > 0 {
//             oa = self.out_pool.active_count();
//             iq = self.in_pool.queued_count();
//             ia = self.in_pool.active_count();
//             oq = self.out_pool.queued_count();
//             ts = self.threads.lock().unwrap().len();
//             info!(
//                 "Stats input queue:{} active:{} output queue:{} active:{} threads:{}",
//                 iq, ia, oq, oa, ts,
//             );
//             thread::sleep(Duration::from_secs(1));
//         }
//     }
//
//     pub fn join(&self) {
//         let clone = self.clone();
//         thread::spawn(move || clone.log_stats());
//         while self.in_pool.active_count()
//             + self.out_pool.active_count()
//             + self.in_pool.queued_count()
//             + self.out_pool.queued_count()
//             + self.threads.lock().unwrap().len()
//             > 0
//         {
//             self.in_pool.join();
//             debug!("Input thread pool joined");
//             self.out_pool.join();
//             debug!("Output thread pool joined");
//             self.threads.lock().unwrap().drain(..).for_each(|x| {
//                 let _ = x.join();
//             });
//             debug!("Threads joined");
//         }
//     }
// }
