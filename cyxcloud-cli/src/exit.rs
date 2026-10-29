//! Stable exit codes (TOFIX177): scripts and the Engine can tell who has to
//! act from the code alone.
//!
//! | Code | Meaning |
//! | --- | --- |
//! | 0 | Done |
//! | 1 | You: a wrong argument, not signed in, no such dataset, refused |
//! | 2 | Network: the server or the node could not be reached in time |
//! | 3 | Server: the server, or the node on this machine, failed |
//!
//! A failure is classified by the first error in its chain that says where
//! it came from (a gRPC status, an HTTP status, a connection error, or an
//! explicit [`Coded`]). An error that names no source is the user's: the
//! CLI's own checks (`bail!`) are about the arguments or the local machine.

use crate::client::ClientError;
use crate::cyxwiz_client::CyxWizError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    User = 1,
    Network = 2,
    Server = 3,
}

impl Kind {
    pub fn code(self) -> i32 {
        self as i32
    }
}

/// An error that carries its exit code.
#[derive(Debug)]
pub struct Coded {
    pub kind: Kind,
    pub message: String,
}

impl std::fmt::Display for Coded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Coded {}

pub fn coded(kind: Kind, message: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(Coded {
        kind,
        message: message.into(),
    })
}

/// Who has to act on a gRPC status.
pub fn grpc_kind(code: tonic::Code) -> Kind {
    use tonic::Code;
    match code {
        Code::Unavailable | Code::DeadlineExceeded | Code::Cancelled => Kind::Network,
        Code::InvalidArgument
        | Code::NotFound
        | Code::AlreadyExists
        | Code::PermissionDenied
        | Code::Unauthenticated
        | Code::FailedPrecondition
        | Code::OutOfRange
        | Code::ResourceExhausted
        | Code::Aborted => Kind::User,
        Code::Ok | Code::Unknown | Code::Internal | Code::Unimplemented | Code::DataLoss => {
            Kind::Server
        }
    }
}

/// Who has to act on an HTTP status that is not a success.
pub fn http_kind(status: u16) -> Kind {
    match status {
        408 | 502 | 503 | 504 => Kind::Network,
        400..=499 => Kind::User,
        _ => Kind::Server,
    }
}

fn reqwest_kind(e: &reqwest::Error) -> Kind {
    if e.is_connect() || e.is_timeout() || e.is_request() {
        Kind::Network
    } else if let Some(s) = e.status() {
        http_kind(s.as_u16())
    } else {
        // A reply that could not be read as what the API promised
        Kind::Server
    }
}

/// The exit code for a failed command.
pub fn classify(err: &anyhow::Error) -> Kind {
    for cause in err.chain() {
        if let Some(c) = cause.downcast_ref::<Coded>() {
            return c.kind;
        }
        if let Some(s) = cause.downcast_ref::<tonic::Status>() {
            return grpc_kind(s.code());
        }
        if cause.downcast_ref::<tonic::transport::Error>().is_some() {
            return Kind::Network;
        }
        if let Some(e) = cause.downcast_ref::<reqwest::Error>() {
            return reqwest_kind(e);
        }
        if let Some(e) = cause.downcast_ref::<CyxWizError>() {
            return match e {
                CyxWizError::Http(e) => reqwest_kind(e),
                CyxWizError::Api { status, .. } => http_kind(*status),
                CyxWizError::InvalidCredentials
                | CyxWizError::AccountNotFound
                | CyxWizError::EmailAlreadyExists => Kind::User,
            };
        }
        if let Some(e) = cause.downcast_ref::<ClientError>() {
            return match e {
                ClientError::Http(e) => reqwest_kind(e),
                ClientError::Api { status, .. } => http_kind(*status),
                ClientError::NotFound(_) | ClientError::Io(_) => Kind::User,
            };
        }
    }
    Kind::User
}

/// The error and its causes on one line. A cause whose words are already in
/// the line is left out: reqwest and thiserror errors repeat their source.
pub fn describe(err: &anyhow::Error) -> String {
    let mut line = String::new();
    for cause in err.chain() {
        let text = cause.to_string();
        if line.contains(&text) {
            continue;
        }
        if !line.is_empty() {
            line.push_str(": ");
        }
        line.push_str(&text);
    }
    line
}

/// What to do next, when the code alone says it.
pub fn next_step(kind: Kind) -> Option<&'static str> {
    match kind {
        Kind::Network => Some("check the address or your network, then run `cyxcloud doctor`"),
        Kind::User | Kind::Server => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_source_of_a_failure_decides_its_code() {
        let unreachable: anyhow::Error = tonic::Status::unavailable("down").into();
        assert_eq!(classify(&unreachable), Kind::Network);
        let refused: anyhow::Error = tonic::Status::unauthenticated("token").into();
        assert_eq!(classify(&refused.context("listing")), Kind::User);
        let broken: anyhow::Error = tonic::Status::data_loss("shard").into();
        assert_eq!(classify(&broken), Kind::Server);
        let api = anyhow::Error::new(CyxWizError::Api {
            status: 500,
            message: String::new(),
        });
        assert_eq!(classify(&api), Kind::Server);
        let wrong = anyhow::Error::new(CyxWizError::InvalidCredentials);
        assert_eq!(classify(&wrong.context("Login failed")), Kind::User);
        assert_eq!(
            classify(&anyhow::anyhow!("give at least one file")),
            Kind::User
        );
        let node = coded(Kind::Server, "the node stopped right away");
        assert_eq!(classify(&node), Kind::Server);
    }

    #[test]
    fn http_statuses_name_who_acts() {
        assert_eq!(http_kind(404), Kind::User);
        assert_eq!(http_kind(409), Kind::User);
        assert_eq!(http_kind(503), Kind::Network);
        assert_eq!(http_kind(500), Kind::Server);
    }

    #[test]
    fn a_cause_already_in_the_line_is_not_repeated() {
        let e = anyhow::anyhow!("operation timed out")
            .context("error sending request: operation timed out")
            .context("Login failed");
        assert_eq!(
            describe(&e),
            "Login failed: error sending request: operation timed out"
        );
    }

    #[test]
    fn codes_are_stable() {
        assert_eq!(Kind::User.code(), 1);
        assert_eq!(Kind::Network.code(), 2);
        assert_eq!(Kind::Server.code(), 3);
    }
}
