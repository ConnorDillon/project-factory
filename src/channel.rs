pub trait Send<T> {
    fn send(&self, msg: T) -> bool;
}

pub trait Recv<T> {
    fn recv(&self) -> Option<T>;
}
