#![allow(dead_code)]
#![allow(unused_imports)]

mod request {
    include!(concat!(env!("OUT_DIR"), "/request.rs"));
}
mod session {
    include!(concat!(env!("OUT_DIR"), "/session.rs"));
}
mod screencast {
    include!(concat!(env!("OUT_DIR"), "/screencast.rs"));
}

pub use request::*;
pub use screencast::*;
pub use session::*;
