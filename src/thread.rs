use std::io::Write;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use log::{debug, error, info};
use threadpool::ThreadPool;

use crate::input::{Input, InputFactory};
use crate::output::Output;
use crate::pre_process::PreProcessor;

#[derive(Clone)]
pub struct Pool<E> {
    pub in_pool: ThreadPool,
    pub out_pool: ThreadPool,
    threads: Arc<Mutex<Vec<JoinHandle<()>>>>,
    pub factory: Arc<InputFactory>,
    pub pre_processor: Arc<PreProcessor>,
    pub exit: E,
}

impl<E> Pool<E>
where
    E: Write + Clone + Send + 'static,
{
    pub fn new(
        in_pool_size: usize,
        out_pool_size: usize,
        pre_processor: PreProcessor,
        exit: E,
    ) -> Pool<E> {
        Pool {
            in_pool: ThreadPool::new(in_pool_size),
            out_pool: ThreadPool::new(out_pool_size),
            factory: Arc::new(InputFactory::new()),
            pre_processor: Arc::new(pre_processor),
            threads: Arc::new(Mutex::new(Vec::new())),
            exit,
        }
    }

    pub fn process_input(&self, input: Input) {
        let task_id = input.task_id;
        let path = input.item_path.clone();
        let is_stdout = input.data.is_stdout();
        let clone = self.clone();
        let job = move || {
            debug!(
                "{}: START Input {:?} data: {:?}",
                input.task_id, path, input.data
            );
            let input_cb = |x| clone.process_input(x);
            let output_cb = |x| clone.process_output(x);
            if let Some(err) = input
                .handle(
                    clone.factory.clone(),
                    clone.pre_processor.clone(),
                    &input_cb,
                    &output_cb,
                )
                .err()
            {
                error!("{}: FINISH Input {:?} error: {:?}", task_id, path, err)
            } else {
                debug!("{}: FINISH Input {:?}", task_id, path);
            }
        };
        if is_stdout {
            let mut guard = self.threads.lock().unwrap();
            guard.push(thread::spawn(job));
        } else {
            self.in_pool.execute(job);
        }
    }

    pub fn process_output(&self, output: Output) {
        let task_id = output.task_id;
        let mut exit = self.exit.clone();
        let path = output.item_path.clone();
        let plugin = output.plugin_name.clone();
        self.out_pool.execute(move || {
            debug!(
                "{}: START Output {:?} data: {:?}",
                output.task_id, path, output.data
            );
            if let Some(err) = output.handle(&mut exit).err() {
                error!(
                    "{}: FINISH Output {:?} plugin: {}, error: {:?}",
                    task_id, path, plugin, err
                )
            } else {
                debug!("{}: FINISH Output {:?} plugin: {}", task_id, path, plugin);
            }
        })
    }

    pub fn log_stats(&self) {
        let mut oa = 1;
        let mut iq = 1;
        let mut ia = 1;
        let mut oq = 1;
        let mut ts = 1;
        while iq + ia + oq + oa + ts > 0 {
            oa = self.out_pool.active_count();
            iq = self.in_pool.queued_count();
            ia = self.in_pool.active_count();
            oq = self.out_pool.queued_count();
            ts = self.threads.lock().unwrap().len();
            info!(
                "Stats input queue:{} active:{} output queue:{} active:{} threads:{}",
                iq, ia, oq, oa, ts,
            );
            thread::sleep(Duration::from_secs(1));
        }
    }

    pub fn join(&self) {
        let clone = self.clone();
        thread::spawn(move || clone.log_stats());
        while self.in_pool.active_count()
            + self.out_pool.active_count()
            + self.in_pool.queued_count()
            + self.out_pool.queued_count()
            + self.threads.lock().unwrap().len()
            > 0
        {
            self.in_pool.join();
            debug!("Input thread pool joined");
            self.out_pool.join();
            debug!("Output thread pool joined");
            self.threads.lock().unwrap().drain(..).for_each(|x| {
                let _ = x.join();
            });
            debug!("Threads joined");
        }
    }
}
