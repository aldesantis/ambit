//! A canned [`Http`] for the self-update suites, so none of them reaches the network.

use std::cell::{Cell, RefCell};
use std::io::Cursor;

use crate::self_update::release::{GetOptions, Http, HttpResponse};

/// One canned answer: a status, an optional `Location`, and a body.
pub struct Canned {
    pub status: u16,
    pub location: Option<String>,
    pub body: Vec<u8>,
}

impl Canned {
    /// A 200 carrying `body`.
    pub fn ok(body: impl Into<Vec<u8>>) -> Self {
        Self {
            status: 200,
            location: None,
            body: body.into(),
        }
    }

    /// A bodiless answer with `status`.
    pub fn status(status: u16) -> Self {
        Self {
            status,
            location: None,
            body: b"Not Found".to_vec(),
        }
    }

    /// A 302 pointing at `location`.
    pub fn redirect(location: &str) -> Self {
        Self {
            status: 302,
            location: Some(location.to_owned()),
            body: Vec::new(),
        }
    }
}

type Route = Box<dyn Fn(&str) -> Result<Canned, String>>;

/// Answers every request through `route`, counting requests and remembering their URLs.
pub struct FakeHttp {
    route: Route,
    pub calls: Cell<usize>,
    pub urls: RefCell<Vec<String>>,
}

impl FakeHttp {
    pub fn new(route: impl Fn(&str) -> Result<Canned, String> + 'static) -> Self {
        Self {
            route: Box::new(route),
            calls: Cell::new(0),
            urls: RefCell::new(Vec::new()),
        }
    }

    /// One that fails the test if it is asked anything.
    pub fn unreachable() -> Self {
        Self::new(|url| panic!("should not be called: {url}"))
    }
}

/// A `.tar.xz` holding `members` (path, bytes), as cargo-dist builds one.
pub fn tar_xz(members: &[(&str, &[u8])]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());

    for (path, bytes) in members {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder
            .append_data(&mut header, path, *bytes)
            .expect("append a member");
    }

    let tarball = builder.into_inner().expect("finish the tarball");
    let mut xz = lzma_rust2::XzWriter::new(Vec::new(), lzma_rust2::XzOptions::with_preset(1))
        .expect("start the xz stream");

    std::io::Write::write_all(&mut xz, &tarball).expect("compress");
    xz.finish().expect("finish the xz stream")
}

/// A `.zip` holding `members` (path, bytes), stored uncompressed.
pub fn zip(members: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

    for (path, bytes) in members {
        writer.start_file(*path, options).expect("start a member");
        std::io::Write::write_all(&mut writer, bytes).expect("write a member");
    }

    writer.finish().expect("finish the zip").into_inner()
}

impl Http for FakeHttp {
    fn get(&self, url: &str, _options: &GetOptions) -> Result<HttpResponse, String> {
        self.calls.set(self.calls.get() + 1);
        self.urls.borrow_mut().push(url.to_owned());

        let canned = (self.route)(url)?;

        Ok(HttpResponse {
            status: canned.status,
            location: canned.location,
            body: Box::new(Cursor::new(canned.body)),
        })
    }
}
