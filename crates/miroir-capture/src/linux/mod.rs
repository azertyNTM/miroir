//! Linux capture: the ScreenCast portal for the user's choice, PipeWire for the frames.

mod stream;

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use futures_util::StreamExt;
use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
use ashpd::desktop::{PersistMode, ResponseError, Session};
use miroir_core::frame::VideoFrame;
use tracing::{debug, info};

pub use stream::PipewireStream;

use crate::{SelectionCancelled, VideoSource};

/// A screen or a window picked by the user in the desktop's own dialog.
pub struct PortalSource {
    stream: PipewireStream,
    /// Closing the session is what makes the desktop stop sharing.
    session: Option<Arc<Session<Screencast>>>,
    runtime: tokio::runtime::Runtime,
}

impl PortalSource {
    /// Shows the desktop's picker, or reuses the previous choice when `restore_token` is still
    /// valid. Returns the token to pass next time.
    ///
    /// Blocks until the user has answered: call it off the UI thread.
    pub fn open(restore_token: Option<&str>) -> anyhow::Result<(Self, Option<String>)> {
        let runtime = new_runtime()?;

        let (session, remote, node_id, new_token) = runtime.block_on(async {
            let proxy = portal().await?;
            let session = proxy.create_session(Default::default()).await?;
            proxy
                .select_sources(
                    &session,
                    SelectSourcesOptions::default()
                        .set_cursor_mode(CursorMode::EMBEDDED)
                        .set_sources(SourceType::MONITOR | SourceType::WINDOW)
                        .set_multiple(false)
                        .set_persist_mode(PersistMode::ExplicitlyRevoked)
                        .set_restore_token(restore_token),
                )
                .await?
                .response()
                .context("the portal refused the capture options")?;
            let streams = match proxy.start(&session, None, Default::default()).await?.response() {
                Ok(streams) => streams,
                Err(ashpd::Error::Response(ResponseError::Cancelled)) => {
                    return Err(SelectionCancelled.into());
                }
                Err(e) => return Err(e.into()),
            };
            let node_id = streams
                .streams()
                .first()
                .context("the portal returned no stream")?
                .pipe_wire_node_id();
            let remote = proxy.open_pipe_wire_remote(&session, Default::default()).await?;
            anyhow::Ok((session, remote, node_id, streams.restore_token().map(str::to_owned)))
        })?;

        info!(node_id, restored = restore_token.is_some(), "portal session started");
        let stream = PipewireStream::connect(Some(remote), node_id)?;

        // The desktop closes the session when the user stops sharing from its own UI or when
        // the captured window is closed.
        let session = Arc::new(session);
        runtime.spawn({
            let session = session.clone();
            let end = stream.ender();
            async move {
                if let Ok(closed) = session.receive_closed().await {
                    std::pin::pin!(closed).next().await;
                    end("sharing was stopped from the desktop");
                }
            }
        });

        Ok((Self { stream, session: Some(session), runtime }, new_token))
    }
}

/// zbus needs a running executor for as long as the session lives.
fn new_runtime() -> std::io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build()
}

/// A connection of our own, not ashpd's process-wide one: that one stays bound to the runtime
/// that first used it and never answers again once that runtime is dropped.
async fn portal() -> anyhow::Result<Screencast> {
    let connection = ashpd::zbus::Connection::session().await.context("no D-Bus session")?;
    Screencast::with_connection(connection).await.context("screen capture portal unavailable")
}

impl VideoSource for PortalSource {
    fn next_frame(&mut self) -> anyhow::Result<Option<VideoFrame>> {
        self.stream.next_frame(Duration::from_millis(100))
    }

    fn stop(&mut self) {
        self.stream.stop();
        if let Some(session) = self.session.take()
            && let Err(e) = self.runtime.block_on(session.close())
        {
            debug!("closing the portal session: {e}");
        }
    }
}

impl Drop for PortalSource {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each capture owns its runtime: the portal must still answer after earlier runtimes are
    /// gone. Needs a desktop session: `cargo test -p miroir-capture -- --ignored`.
    #[test]
    #[ignore = "needs a running desktop portal"]
    fn portal_answers_from_successive_runtimes() {
        for round in 1..=3 {
            let runtime = new_runtime().unwrap();
            let answer = runtime.block_on(async {
                tokio::time::timeout(Duration::from_secs(3), async {
                    anyhow::Ok(portal().await?.available_source_types().await?)
                })
                .await
            });
            assert!(matches!(answer, Ok(Ok(_))), "round {round}: {answer:?}");
        }
    }
}
