//! The fuzzy filename search of the gallery.

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

/// One query: the names to search and the channel of the ranked indices.
struct Request {
    query: String,
    names: Arc<Vec<String>>,
    reply: Sender<Vec<usize>>,
}

/// A long-lived filter thread with one `Matcher`. The worker copies the names only when they
/// change.
#[derive(Default)]
pub(crate) struct FilterWorker {
    requests: Option<Sender<Request>>,
    names: Arc<Vec<String>>,
}

impl FilterWorker {
    /// The reply holds the indices, best first. A new query replaces the receiver of the old one,
    /// and a late old result goes nowhere. The thread ends when the worker drops.
    pub(crate) fn submit(&mut self, query: &str, filenames: &[String]) -> Receiver<Vec<usize>> {
        if self.names.as_slice() != filenames {
            self.names = Arc::new(filenames.to_vec());
        }
        let (reply, results) = mpsc::channel();
        let mut request = Request {
            query: query.to_owned(),
            names: Arc::clone(&self.names),
            reply,
        };
        loop {
            let requests = self.requests.get_or_insert_with(spawn_thread);
            match requests.send(request) {
                Ok(()) => return results,
                // The thread is gone (a panic): start a new one and send again.
                Err(mpsc::SendError(unsent)) => {
                    request = unsent;
                    self.requests = None;
                }
            }
        }
    }

    #[cfg(test)]
    fn names(&self) -> &Arc<Vec<String>> {
        &self.names
    }
}

fn spawn_thread() -> Sender<Request> {
    let (requests, queue) = mpsc::channel::<Request>();
    thread::spawn(move || {
        let mut matcher = Matcher::new(Config::DEFAULT);
        let mut buf = Vec::new();
        while let Ok(mut request) = queue.recv() {
            // Waiting queries are stale. Only the newest one runs.
            while let Ok(newer) = queue.try_recv() {
                request = newer;
            }
            let results = rank(&request.query, &request.names, &mut matcher, &mut buf);
            #[expect(
                clippy::let_underscore_must_use,
                reason = "a closed receiver means that nobody waits for the result"
            )]
            let _ = request.reply.send(results);
        }
    });
    requests
}

/// Best score first. An empty query keeps the order.
fn rank(
    query: &str,
    filenames: &[String],
    matcher: &mut Matcher,
    buf: &mut Vec<char>,
) -> Vec<usize> {
    if query.is_empty() {
        return (0..filenames.len()).collect();
    }

    let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
    let mut scored: Vec<(usize, u32)> = Vec::new();

    for (i, name) in filenames.iter().enumerate() {
        let haystack = Utf32Str::new(name, buf);
        if let Some(score) = pattern.score(haystack, matcher) {
            scored.push((i, score));
        }
        buf.clear();
    }

    scored.sort_by_key(|(_, score)| std::cmp::Reverse(*score));
    scored.into_iter().map(|(i, _)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::{FilterWorker, Request, rank, spawn_thread};
    use nucleo_matcher::{Config, Matcher};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    /// One query with a new matcher, as the old thread per key press did it.
    fn filter(query: &str, filenames: &[String]) -> Vec<usize> {
        rank(
            query,
            filenames,
            &mut Matcher::new(Config::DEFAULT),
            &mut Vec::new(),
        )
    }

    /// Wait at most 2 s, and a hang fails the test.
    fn answer(results: &std::sync::mpsc::Receiver<Vec<usize>>) -> Vec<usize> {
        results.recv_timeout(Duration::from_secs(2)).unwrap()
    }

    #[test]
    fn empty_query_preserves_input_order() {
        let filenames = vec!["b.png".into(), "a.png".into()];
        assert_eq!(filter("", &filenames), vec![0, 1]);
    }

    #[test]
    fn fuzzy_query_returns_only_matches() {
        let filenames = vec!["vacation.jpg".into(), "report.png".into()];
        assert_eq!(filter("vac", &filenames), vec![0]);
    }

    #[test]
    fn worker_copies_list_once() {
        let filenames: Vec<String> = vec!["vacation.jpg".into(), "report.png".into()];
        let mut worker = FilterWorker::default();

        let first = worker.submit("vac", &filenames);
        assert_eq!(answer(&first), vec![0]);
        let list = Arc::clone(worker.names());
        let second = worker.submit("rep", &filenames);
        assert_eq!(answer(&second), vec![1]);
        assert!(
            Arc::ptr_eq(&list, worker.names()),
            "the second query copied the list"
        );

        let longer: Vec<String> = vec![
            "vacation.jpg".into(),
            "report.png".into(),
            "rep2.png".into(),
        ];
        let third = worker.submit("rep", &longer);
        assert!(
            !Arc::ptr_eq(&list, worker.names()),
            "a changed list needs a new copy"
        );
        assert_eq!(answer(&third), vec![1, 2]);
    }

    #[test]
    fn each_query_gets_own_reply() {
        let filenames: Vec<String> = vec!["vacation.jpg".into(), "report.png".into()];
        let mut worker = FilterWorker::default();

        let old = worker.submit("vac", &filenames);
        let new = worker.submit("rep", &filenames);

        assert_eq!(answer(&new), vec![1]);
        assert!(new.recv_timeout(Duration::from_millis(50)).is_err());
        drop(old);
    }

    #[test]
    fn restarts_dead_thread() {
        let filenames: Vec<String> = vec!["vacation.jpg".into(), "report.png".into()];
        let mut worker = FilterWorker::default();
        let (requests, queue) = std::sync::mpsc::channel();
        drop(queue);
        worker.requests = Some(requests);

        assert_eq!(answer(&worker.submit("rep", &filenames)), vec![1]);
    }

    #[test]
    fn queued_queries_answer_or_drop() {
        let names = Arc::new(vec!["vacation.jpg".to_owned(), "report.png".to_owned()]);
        let requests = spawn_thread();
        let replies = ["v", "va", "rep"].map(|query| {
            let (reply, results) = std::sync::mpsc::channel();
            let request = Request {
                query: query.to_owned(),
                names: Arc::clone(&names),
                reply,
            };
            requests.send(request).unwrap();
            results
        });

        assert_eq!(answer(&replies[2]), vec![1]);
        // The thread runs the queries in order. Each older query has its answer or a closed
        // channel.
        for (results, expected) in replies[..2].iter().zip([vec![0], vec![0]]) {
            match results.try_recv() {
                Ok(ranked) => assert_eq!(ranked, expected),
                Err(error) => assert_eq!(error, std::sync::mpsc::TryRecvError::Disconnected),
            }
        }
    }

    #[test]
    fn better_match_ranks_first() {
        let filenames = vec!["xrxexpx.png".into(), "report.png".into()];
        assert_eq!(filter("rep", &filenames), vec![1, 0]);
    }

    /// The old way (a list copy, a thread, a new matcher) against the worker on 10 000 names:
    /// `cargo test --release -- --ignored search::`.
    #[test]
    #[ignore = "timing"]
    fn time_per_key_press() {
        let filenames: Vec<String> = (0..10_000)
            .map(|index| format!("IMG_{index:05}_holiday_{}.jpg", index * 7 % 1000))
            .collect();
        let queries = [
            "i", "im", "img", "img_", "img_1", "img_12", "img_123", "img_1234",
        ];
        let rounds = 20_u32;

        // `ui` is the part on the UI thread: the copy and the thread start.
        let (mut total, mut ui) = (Duration::ZERO, Duration::ZERO);
        for _ in 0..rounds {
            for query in queries {
                let start = Instant::now();
                let (query, names) = (query.to_owned(), filenames.clone());
                let (tx, rx) = std::sync::mpsc::channel();
                std::thread::spawn(move || tx.send(filter(&query, &names)).unwrap());
                ui += start.elapsed();
                rx.recv().unwrap();
                total += start.elapsed();
            }
        }
        let (old, old_ui) = (total / (rounds * 8), ui / (rounds * 8));

        let mut worker = FilterWorker::default();
        let (mut total, mut ui) = (Duration::ZERO, Duration::ZERO);
        for _ in 0..rounds {
            for query in queries {
                let start = Instant::now();
                let results = worker.submit(query, &filenames);
                ui += start.elapsed();
                answer(&results);
                total += start.elapsed();
            }
        }
        let (new, new_ui) = (total / (rounds * 8), ui / (rounds * 8));
        eprintln!(
            "bench search: per key press, old {old:?} (UI thread {old_ui:?}), worker {new:?} (UI thread {new_ui:?})"
        );
    }
}
