#![feature(command_access, thread_id_value)]

use std::env;
use std::fs::{self, File};
use std::io::{self, Read, Stdout, Write};
use std::path::PathBuf;

use env_logger::Builder;
use getopts::Options;
use log::debug;
use serde_yaml::from_reader;

use crate::input::InputData;
use crate::plugin::Config;
use crate::thread::Pool;

mod input;
mod output;
mod plugin;
mod pre_process;
#[allow(dead_code)]
mod thread;
mod walk;

fn main() {
    init_logger();
    let opts = set_opts();
    let args: Vec<String> = env::args().collect();
    let params = read_params(&opts, &args);
    if params.help {
        print!("{}", opts.usage("Usage: factory [options]"));
    } else if let (Some(cpath), Some(ypath)) = (params.config, params.yara) {
        let cfile = File::open(cpath).unwrap();
        let conf: Config = from_reader(cfile).unwrap();
        debug!("Config: {:?}", conf);
        let mut rules = String::new();
        File::open(ypath)
            .unwrap()
            .read_to_string(&mut rules)
            .unwrap();
        //let mut compiler = Compiler::new().unwrap();
        //compiler.add_rules_file(ypath).unwrap();
        //let rules = compiler.compile_rules().unwrap();
        execute(params.input, conf, rules, Output(io::stdout())).unwrap();
    } else {
        print!("{}", opts.usage("Usage: factory [options]"));
    }
}

fn execute<E>(input: Option<PathBuf>, config: Config, rules: String, exit: E) -> io::Result<()>
where
    E: Write + Clone + Send + 'static,
{
    let cpus = num_cpus::get();
    let mut pool = Pool::new(config, rules, exit);
    pool.add_input_threads(cpus);
    pool.add_output_threads(cpus * 2);
    let input_path = match input {
        Some(p) => {
            let mut path = env::current_dir()?;
            path.push(p);
            Some(path)
        }
        None => None,
    };
    let working_dir = plugin::gen_path()?;
    fs::create_dir(&working_dir).unwrap();
    env::set_current_dir(&working_dir)?;
    if let Some(path) = input_path {
        if path.is_dir() {
            walk::walk_dir(path, "".into(), |p, ip| {
                let inp = pool.factory.new_input(ip, InputData::File(p, false));
                pool.input_sender.send(inp).unwrap();
            })?;
        } else {
            pool.input_sender
                .send(pool.factory.new_input("", InputData::File(path, false)))
                .unwrap();
        }
    } else {
        pool.input_sender
            .send(pool.factory.new_input("", InputData::Stdin(io::stdin())))
            .unwrap();
    }
    pool.join().unwrap();
    env::set_current_dir(&working_dir.parent().unwrap())?;
    fs::remove_dir_all(working_dir).unwrap();
    Ok(())
}

fn set_opts() -> Options {
    let mut opts = Options::new();
    opts.optflag("h", "help", "Show this help information.");
    opts.optopt(
        "y",
        "yara",
        "Path to the yara rules file (required)",
        "PATH",
    );
    opts.optopt("c", "config", "Path to the config file (required)", "PATH");
    opts.optopt(
        "i",
        "input",
        "Path to the input file (will read from stdin if not specified)",
        "PATH",
    );
    opts
}

fn read_params(opts: &Options, args: &Vec<String>) -> Params {
    let matches = opts.parse(&args[1..]).unwrap();
    Params {
        help: matches.opt_present("help"),
        config: matches.opt_get("config").unwrap(),
        yara: matches.opt_get("yara").unwrap(),
        input: matches.opt_get("input").unwrap(),
    }
}

struct Params {
    help: bool,
    config: Option<PathBuf>,
    yara: Option<PathBuf>,
    input: Option<PathBuf>,
}

fn init_logger() {
    Builder::from_default_env()
        .format(|buf, record| {
            writeln!(
                buf,
                "[{} {} Thread({})] {}",
                buf.timestamp(),
                record.level(),
                std::thread::current().id().as_u64(),
                record.args()
            )
        })
        .init();
}

struct Output(Stdout);

impl Clone for Output {
    fn clone(&self) -> Output {
        Output(io::stdout())
    }
}

impl Write for Output {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.0.write_all(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}
