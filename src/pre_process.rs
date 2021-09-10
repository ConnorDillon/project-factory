use std::io::{self, Chain, Cursor, Read};
use std::path::PathBuf;
use std::sync::Mutex;

use log::{debug, info, warn};
use yara::{Metadata, MetadataValue, Rules};

use crate::output::TaskId;
use crate::plugin::{Config, FileType, PreppedPlugin};

pub struct PreProcessedInput<T> {
    pub task_id: TaskId,
    pub item_path: PathBuf,
    pub item_type: String,
    pub plugin: PreppedPlugin,
    pub data: T,
}

pub struct PreProcessor {
    pub conf: Config,
    pub rules: Mutex<Rules>,
}

impl PreProcessor {
    pub fn new(conf: Config, rules: Rules) -> PreProcessor {
        PreProcessor {
            conf,
            rules: Mutex::new(rules),
        }
    }

    pub fn pre_process<R: Read>(
        &self,
        task_id: TaskId,
        item_path: PathBuf,
        file_path: Option<&PathBuf>,
        mut data: R,
    ) -> io::Result<Option<PreProcessedInput<Chain<Cursor<Vec<u8>>, R>>>> {
        let mut buf = Vec::with_capacity(4096);
        (&mut data).take(4096).read_to_end(&mut buf)?;
        let get_file_type = self.get_file_type(&buf);
        match get_file_type {
            Some(item_type) => match self.conf.get(&item_type) {
                Some(plugin) => {
                    let pplugin = plugin.prep(file_path)?;
                    debug!("{}: Prepped plugin: {:?}", task_id, pplugin);
                    info!(
                        "{}: Processing {:?} type: {} with plugin: {}",
                        task_id, item_path, item_type, pplugin.plugin_name
                    );
                    Ok(Some(PreProcessedInput {
                        task_id,
                        item_path,
                        item_type,
                        plugin: pplugin,
                        data: Cursor::new(buf).chain(data),
                    }))
                }
                None => {
                    warn!(
                        "{}: File type for {:?} not included in config: {}",
                        task_id, item_path, item_type
                    );
                    Ok(None)
                }
            },
            None => {
                warn!(
                    "{}: File type for {:?} was not determined",
                    task_id, item_path
                );
                Ok(None)
            }
        }
    }

    fn get_file_type(&self, head: &[u8]) -> Option<FileType> {
        self.rules
            .lock()
            .unwrap()
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
}

fn meta_string(meta: &Metadata) -> Option<String> {
    match meta.value {
        MetadataValue::String(x) => Some(x.into()),
        _ => None,
    }
}
