use std::io::Write;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use log::{debug, error};
use threadpool::ThreadPool;

use crate::input::Input;
use crate::output::Output;
use crate::task::TaskFactory;

#[derive(Clone)]
pub struct Pool<E> {
    pub in_pool: ThreadPool,
    pub out_pool: ThreadPool,
    pub threads: Arc<Mutex<Vec<JoinHandle<()>>>>,
    pub factory: Arc<TaskFactory>,
    pub exit: E,
}

impl<E> Pool<E>
where
    E: Write + Clone + Send + 'static,
{
    pub fn new(
        in_pool_size: usize,
        out_pool_size: usize,
        factory: TaskFactory,
        exit: E,
    ) -> Pool<E> {
        Pool {
            in_pool: ThreadPool::new(in_pool_size),
            out_pool: ThreadPool::new(out_pool_size),
            factory: Arc::new(factory),
            threads: Arc::new(Mutex::new(Vec::new())),
            exit,
        }
    }

    pub fn process_input(&self, input: Input) {
        let path = input.item_path.clone();
        let is_stdout = input.data.is_stdout();
        let clone = self.clone();
        let job = move || {
            debug!("Processing {:?}", input);
            let input_cb = |x| clone.process_input(x);
            let output_cb = |x| clone.process_output(x);
            if let Some(err) = input
                .handle(clone.factory.clone(), &input_cb, &output_cb)
                .err()
            {
                error!("Input thread for {:?} finished with error: {:?}", path, err)
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
        let mut exit = self.exit.clone();
        let path = output.item_path.clone();
        let plugin = output.plugin_name.clone();
        self.out_pool.execute(move || {
            debug!("Processing {:?}", output);
            if let Some(err) = output.handle(&mut exit).err() {
                error!(
                    "Output thread for {} {:?} finished with error: {:?}",
                    plugin, path, err
                )
            }
        })
    }

    pub fn join(&self) {
        while self.in_pool.active_count()
            + self.out_pool.active_count()
            + self.in_pool.queued_count()
            + self.out_pool.queued_count()
            + self.threads.lock().unwrap().len()
            > 0
        {
            self.in_pool.join();
            self.out_pool.join();
            self.threads.lock().unwrap().drain(..).for_each(|x| {
                let _ = x.join();
            });
        }
    }
}
