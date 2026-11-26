#![allow(dead_code, clippy::new_without_default)]
//! A GitHub that wiremock plays, driven from plain (blocking) tests.
use claude_sync_github::{Client, Config, Tokens};
use serde_json::{json, Value};
use std::time::Duration;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

pub struct Srv {
    rt: tokio::runtime::Runtime,
    pub server: MockServer,
}

impl Srv {
    pub fn new() -> Srv {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let server = rt.block_on(MockServer::start());
        Srv { rt, server }
    }

    pub fn uri(&self) -> String {
        self.server.uri()
    }

    pub fn mount(&self, m: Mock) {
        self.rt.block_on(m.mount(&self.server));
    }

    pub fn requests(&self) -> Vec<Request> {
        self.rt
            .block_on(self.server.received_requests())
            .unwrap_or_default()
    }

    pub fn config(&self) -> Config {
        Config {
            api: self.uri(),
            login: self.uri(),
            relay: self.uri(),
            releases: format!("{}/releases", self.uri()),
            client_id: "Iv1.test".into(),
            retry_delay: Duration::from_millis(1),
        }
    }

    /// A signed-in client.
    pub fn client(&self) -> Client {
        let c = Client::new(self.config()).unwrap();
        c.set_tokens(Tokens::plain("tok"));
        c
    }

    /// Answer `verb path` exactly once with `status` and `body`, for a bearer `tok`.
    pub fn once(&self, verb: &str, p: &str, status: u16, body: Value) {
        self.mount(
            Mock::given(method(verb))
                .and(path(p))
                .and(header("authorization", "Bearer tok"))
                .respond_with(ResponseTemplate::new(status).set_body_json(body))
                .expect(1),
        );
    }

    /// Like `once` with no body in the answer (204).
    pub fn once_empty(&self, verb: &str, p: &str) {
        self.mount(
            Mock::given(method(verb))
                .and(path(p))
                .and(header("authorization", "Bearer tok"))
                .respond_with(ResponseTemplate::new(204))
                .expect(1),
        );
    }
}

pub fn repo_json(name: &str) -> Value {
    json!({"name": name, "full_name": format!("o/{name}"), "private": true,
           "owner": {"login": "o"}, "permissions": {"admin": true, "push": true, "pull": true}})
}
