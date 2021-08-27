use std::io;
use std::path::PathBuf;

pub fn walk_dir<T: Fn(PathBuf, PathBuf)>(
    dir: PathBuf,
    parent_path: PathBuf,
    send: T,
) -> io::Result<()> {
    let root_depth = dir.iter().count();
    let mut dirs = vec![dir];
    while let Some(dir) = dirs.pop() {
        for entry in dir.read_dir()? {
            let path = entry?.path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.is_file() {
                let mut item_path = parent_path.clone();
                item_path.extend(path.iter().skip(root_depth));
                send(path, item_path);
            }
        }
    }
    Ok(())
}
