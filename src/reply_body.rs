//This code is copied from linkerd2 proxy project (everything here was invented by great linkerd's engieeners)
//I used code from https://github.com/linkerd/linkerd2-proxy/blob/main/linkerd/http/retry/src/replay.rs 
//I was not smart enoght to write such brilant piece of code, so borrowed it with love.
//More about https://linkerd.io/2021/10/26/how-linkerd-retries-http-requests-with-bodies/ 
//Yes, I also copied the comments, because are great.
use bytes::{Buf, BufMut, Bytes, BytesMut};
use http::HeaderMap;
use http_body::{Body, SizeHint};
use std::{collections::VecDeque, io::IoSlice, pin::Pin, sync::Arc, task::Context, task::Poll};
use parking_lot::Mutex;
use thiserror::Error;
pub type Error = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug, Error)]
#[error("replay body discarded after reaching maximum buffered bytes limit")]
pub struct Capped;

#[derive(Debug)]
pub enum Data {
    Initial(Bytes),
    Replay(BufList),
}

#[derive(Clone, Debug, Default)]
pub struct BufList {
    bufs: VecDeque<Bytes>,
}

#[derive(Debug)]
pub struct ReplayBody<B> {
    state: Option<BodyState<B>>,
    shared: Arc<SharedState<B>>,
    replay_body: bool,
    replay_trailers: bool,
}

#[derive(Debug)]
struct BodyState<B> {
    buf: BufList,
    trailers: Option<HeaderMap>,
    rest: Option<B>,
    is_completed: bool,
    max_bytes: usize,
}
#[derive(Debug)]
struct SharedState<B> {
    body: Mutex<Option<BodyState<B>>>,
    /// Did the initial body return `true` from `is_end_stream` before it was
    /// ever polled? If so, always return `true`; the body is completely empty.
    ///
    /// We store this separately so that clones of a totally empty body can
    /// always return `true` from `is_end_stream` even when they don't own the
    /// shared state.
    was_empty: bool,

    orig_size_hint: SizeHint,
}

impl<B> BodyState<B> {
    #[inline]
    fn is_capped(&self) -> bool {
        self.max_bytes == 0
    }
}

impl<B: Body> ReplayBody<B> {
    /// Wraps an initial `Body` in a `ReplayBody`.
    ///
    /// In order to prevent unbounded buffering, this takes a maximum number of bytes to buffer as a
    /// second parameter. If more than than that number of bytes would be buffered, the buffered
    /// data is discarded and any subsequent clones of this body will fail. However, the *currently
    /// active* clone of the body is allowed to continue without erroring. It will simply stop
    /// buffering any additional data for retries.
    ///
    /// If the body has a size hint with a lower bound greater than `max_bytes`, the original body
    /// is returned in the error variant.
    pub fn try_new(body: B, max_bytes: usize) -> Result<Self, B> {
        let orig_size_hint = body.size_hint();
        if orig_size_hint.lower() > max_bytes as u64 {
            return Err(body);
        }

        Ok(Self {
            shared: Arc::new(SharedState {
                body: Mutex::new(None),
                orig_size_hint,
                was_empty: body.is_end_stream(),
            }),
            state: Some(BodyState {
                buf: Default::default(),
                trailers: None,
                rest: Some(body),
                is_completed: false,
                max_bytes: max_bytes + 1,
            }),
            // The initial `ReplayBody` has nothing to replay
            replay_body: false,
            replay_trailers: false,
        })
    }

    /// Mutably borrows the body state if this clone currently owns it,
    /// or else tries to acquire it from the shared state.
    ///
    /// # Panics
    ///
    /// This panics if another clone has currently acquired the state, based on
    /// the assumption that a retry body will not be polled until the previous
    /// request has been dropped.
    fn acquire_state<'a>(
        state: &'a mut Option<BodyState<B>>,
        shared: &Mutex<Option<BodyState<B>>>,
    ) -> &'a mut BodyState<B> {
        state.get_or_insert_with(|| shared.lock().take().expect("missing body state"))
    }

    /// Returns `true` if the body previously exceeded the configured maximum
    /// length limit.
    ///
    /// If this is true, the body is now empty, and the request should *not* be
    /// retried with this body.
    pub fn is_capped(&self) -> bool {
        self.state
            .as_ref()
            .map(BodyState::is_capped)
            .unwrap_or_else(|| {
                self.shared
                    .body
                    .lock()
                    .as_ref()
                    .expect("if our `state` was `None`, the shared state must be `Some`")
                    .is_capped()
            })
    }
}

impl<B> Body for ReplayBody<B>
where
    B: Body + Unpin,
    B::Error: Into<Error>,
{
    type Data = Data;
    type Error = Error;

    fn poll_data(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Self::Data, Self::Error>>> {
        let this = self.get_mut();
        let state = Self::acquire_state(&mut this.state, &this.shared.body);
        // Move these out to avoid mutable borrow issues in the `map` closure
        // when polling the inner body.

        // If we haven't replayed the buffer yet, and its not empty, return the
        // buffered data first.
        if this.replay_body {
            if state.buf.has_remaining() {
                // Don't return the buffered data again on the next poll.
                this.replay_body = false;
                return Poll::Ready(Some(Ok(Data::Replay(state.buf.clone()))));
            }

            if state.is_capped() {
                return Poll::Ready(Some(Err(Capped.into())));
            }
        }

        // If the inner body has previously ended, don't poll it again.
        //
        // NOTE(eliza): we would expect the inner body to just happily return
        // `None` multiple times here, but `hyper::Body::channel` (which we use
        // in the tests) will panic if it is polled after returning `None`, so
        // we have to special-case this. :/
        if state.is_completed {
            return Poll::Ready(None);
        }

        // Poll the inner body for more data. If the body has ended, remember
        // that so that future clones will not try polling it again (as
        // described above).
        let mut data = {
            // Get access to the initial body. If we don't have access to the
            // inner body, there's no more work to do.
            let rest = match state.rest.as_mut() {
                Some(rest) => rest,
                None => return Poll::Ready(None),
            };

            match futures::ready!(Pin::new(rest).poll_data(cx)) {
                Some(Ok(data)) => data,
                Some(Err(e)) => return Poll::Ready(Some(Err(e.into()))),
                None => {
                    state.is_completed = true;
                    return Poll::Ready(None);
                }
            }
        };

        // If we have buffered the maximum number of bytes, allow *this* body to
        // continue, but don't buffer any more.
        let length = data.remaining();
        state.max_bytes = state.max_bytes.saturating_sub(length);
        let chunk = if state.is_capped() {
            // If there's data in the buffer, discard it now, since we won't
            // allow any clones to have a complete body.
            if state.buf.has_remaining() {
                state.buf = Default::default();
            }
            data.copy_to_bytes(length)
        } else {
            // Buffer and return the bytes.
            state.buf.push_chunk(data)
        };

        Poll::Ready(Some(Ok(Data::Initial(chunk))))
    }

    fn poll_trailers(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<Option<HeaderMap>, Self::Error>> {
        let this = self.get_mut();
        let state = Self::acquire_state(&mut this.state, &this.shared.body);


        if this.replay_trailers {
            this.replay_trailers = false;
            if let Some(ref trailers) = state.trailers {
                return Poll::Ready(Ok(Some(trailers.clone())));
            }
        }

        if let Some(rest) = state.rest.as_mut() {
            // If the inner body has previously ended, don't poll it again.
            if !rest.is_end_stream() {
                let res = futures::ready!(Pin::new(rest).poll_trailers(cx)).map(|tlrs| {
                    if state.trailers.is_none() {
                        state.trailers = tlrs.clone();
                    }
                    tlrs
                });
                return Poll::Ready(res.map_err(Into::into));
            }
        }

        Poll::Ready(Ok(None))
    }

    fn is_end_stream(&self) -> bool {
        // if the initial body was EOS as soon as it was wrapped, then we are
        // empty.
        if self.shared.was_empty {
            return true;
        }

        let is_inner_eos = self
            .state
            .as_ref()
            .and_then(|state| state.rest.as_ref().map(Body::is_end_stream))
            .unwrap_or(false);

        // if this body has data or trailers remaining to play back, it
        // is not EOS
        !self.replay_body && !self.replay_trailers
            // if we have replayed everything, the initial body may
            // still have data remaining, so ask it
            && is_inner_eos
    }

    #[inline]
    fn size_hint(&self) -> SizeHint {
        // If this clone isn't holding the body, return the original size hint.
        let state = match self.state.as_ref() {
            Some(state) => state,
            None => return self.shared.orig_size_hint.clone(),
        };

        // Otherwise, if we're holding the state but have dropped the inner
        // body, the entire body is buffered so we know the exact size hint.
        let buffered = state.buf.remaining() as u64;
        let rest_hint = match state.rest.as_ref() {
            Some(rest) => rest.size_hint(),
            None => return SizeHint::with_exact(buffered),
        };

        // Otherwise, add the inner body's size hint to the amount of buffered
        // data. An upper limit is only set if the inner body has an upper
        // limit.
        let mut hint = SizeHint::default();
        hint.set_lower(buffered + rest_hint.lower());
        if let Some(rest_upper) = rest_hint.upper() {
            hint.set_upper(buffered + rest_upper);
        }
        hint
    }
}

impl Buf for Data {
    #[inline]
    fn remaining(&self) -> usize {
        match self {
            Data::Initial(buf) => buf.remaining(),
            Data::Replay(bufs) => bufs.remaining(),
        }
    }

    #[inline]
    fn chunk(&self) -> &[u8] {
        match self {
            Data::Initial(buf) => buf.chunk(),
            Data::Replay(bufs) => bufs.chunk(),
        }
    }

    #[inline]
    fn chunks_vectored<'iovs>(&'iovs self, iovs: &mut [IoSlice<'iovs>]) -> usize {
        match self {
            Data::Initial(buf) => buf.chunks_vectored(iovs),
            Data::Replay(bufs) => bufs.chunks_vectored(iovs),
        }
    }

    #[inline]
    fn advance(&mut self, amt: usize) {
        match self {
            Data::Initial(buf) => buf.advance(amt),
            Data::Replay(bufs) => bufs.advance(amt),
        }
    }

    #[inline]
    fn copy_to_bytes(&mut self, len: usize) -> Bytes {
        match self {
            Data::Initial(buf) => buf.copy_to_bytes(len),
            Data::Replay(bufs) => bufs.copy_to_bytes(len),
        }
    }
}

impl BufList {
    fn push_chunk(&mut self, mut data: impl Buf) -> Bytes {
        let len = data.remaining();
        // `data` is (almost) certainly a `Bytes`, so `copy_to_bytes` should
        // internally be a cheap refcount bump almost all of the time.
        // But, if it isn't, this will copy it to a `Bytes` that we can
        // now clone.
        let bytes = data.copy_to_bytes(len);
        // Buffer a clone of the bytes read on this poll.
        self.bufs.push_back(bytes.clone());
        // Return the bytes
        bytes
    }
}

impl Buf for BufList {
    fn remaining(&self) -> usize {
        self.bufs.iter().map(Buf::remaining).sum()
    }

    fn chunk(&self) -> &[u8] {
        self.bufs.front().map(Buf::chunk).unwrap_or(&[])
    }

    fn chunks_vectored<'iovs>(&'iovs self, iovs: &mut [IoSlice<'iovs>]) -> usize {
        // Are there more than zero iovecs to write to?
        if iovs.is_empty() {
            return 0;
        }

        // Loop over the buffers in the replay buffer list, and try to fill as
        // many iovecs as we can from each buffer.
        let mut filled = 0;
        for buf in &self.bufs {
            filled += buf.chunks_vectored(&mut iovs[filled..]);
            if filled == iovs.len() {
                return filled;
            }
        }

        filled
    }

    fn advance(&mut self, mut amt: usize) {
        while amt > 0 {
            let rem = self.bufs[0].remaining();
            // If the amount to advance by is less than the first buffer in
            // the buffer list, advance that buffer's cursor by `amt`,
            // and we're done.
            if rem > amt {
                self.bufs[0].advance(amt);
                return;
            }

            // Otherwise, advance the first buffer to its end, and
            // continue.
            self.bufs[0].advance(rem);
            amt -= rem;

            self.bufs.pop_front();
        }
    }

    fn copy_to_bytes(&mut self, len: usize) -> Bytes {
        // If the length of the requested `Bytes` is <= the length of the front
        // buffer, we can just use its `copy_to_bytes` implementation (which is
        // just a reference count bump).
        match self.bufs.front_mut() {
            Some(first) if len <= first.remaining() => {
                let buf = first.copy_to_bytes(len);
                // If we consumed the first buffer, also advance our "cursor" by
                // popping it.
                if first.remaining() == 0 {
                    self.bufs.pop_front();
                }

                buf
            }
            _ => {
                assert!(len <= self.remaining(), "`len` greater than remaining");
                let mut buf = BytesMut::with_capacity(len);
                buf.put(self.take(len));
                buf.freeze()
            }
        }
    }
}

impl<B> Clone for ReplayBody<B> {
    fn clone(&self) -> Self {
        Self {
            state: None,
            shared: self.shared.clone(),
            // The clone should try to replay from the shared state before
            // reading any additional data from the initial body.
            replay_body: true,
            replay_trailers: true,
        }
    }
}

impl<B> Drop for ReplayBody<B> {
    fn drop(&mut self) {
        // If this clone owned the shared state, put it back.
        if let Some(state) = self.state.take() {
            *self.shared.body.lock() = Some(state);
        }
    }
}