use std::sync::mpsc::Receiver;

fn blocks_the_current_thread(receiver: &Receiver<()>) {
    let _ = receiver.recv();
}
