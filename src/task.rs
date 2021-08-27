use std::io::{self, Chain, Cursor, Read};
use std::path::PathBuf;

use log::{debug, info, warn};
use yara::{Metadata, MetadataValue, Rules};

use crate::plugin::{Config, FileType, PreppedPlugin};

pub struct Task<T> {
    pub item_path: PathBuf,
    pub item_type: String,
    pub plugin: PreppedPlugin,
    pub data: T,
}

pub struct TaskFactory {
    pub conf: Config,
    pub rules: Rules,
}

impl TaskFactory {
    pub fn new(conf: Config, rules: Rules) -> TaskFactory {
        TaskFactory { conf, rules }
    }

    pub fn new_task<R: Read>(
        &self,
        item_path: PathBuf,
        file_path: Option<&PathBuf>,
        mut data: R,
    ) -> io::Result<Option<Task<Chain<Cursor<Vec<u8>>, R>>>> {
        let mut buf = Vec::with_capacity(4096);
        (&mut data).take(4096).read_to_end(&mut buf)?;
        match self.get_file_type(&buf) {
            Some(item_type) => match self.conf.get(&item_type) {
                Some(plugin) => {
                    let pplugin = plugin.prep(file_path)?;
                    debug!("Prepped plugin: {:?}", pplugin);
                    info!(
                        "Processing {:?} (type: {}) with {}",
                        item_path, item_type, pplugin.plugin_name
                    );
                    Ok(Some(Task {
                        item_path,
                        item_type,
                        plugin: pplugin,
                        data: Cursor::new(buf).chain(data),
                    }))
                }
                None => {
                    warn!(
                        "File type for {:?} not included in config: {}",
                        item_path, item_type
                    );
                    Ok(None)
                }
            },
            None => {
                warn!("File type for {:?} was not determined", item_path);
                Ok(None)
            }
        }
    }

    fn get_file_type(&self, head: &[u8]) -> Option<FileType> {
        self.rules
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
