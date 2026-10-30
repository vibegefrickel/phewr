//! The fuzzy filename search of the gallery.

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

/// One query for the filter thread: the names to search and where to send the ranked indices.
struct Request {
    query: String,
    names: Arc<Vec<String>>,
    reply: Sender<Vec<usize>>,
}

/// A long-lived filter thread. It keeps the one `Matcher` and takes queries through a channel. The
/// worker holds the names as an `Arc` and copies the list only when it changed since the last query.
#[derive(Default)]
pub(crate) struct FilterWorker {
    requests: Option<Sender<Request>>,
    names: Arc<Vec<String>>,
}

impl FilterWorker {
    /// Start a search of `filenames` for `query`. The reply is one `Vec` of indices, best first.
    /// A new query replaces the receiver of the old one, so the late result of an old query goes
    /// nowhere. The thread starts at the first query and ends when the worker drops.
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

/// Start the filter thread, so that one `Matcher` serves every query until the worker drops.
fn spawn_thread() -> Sender<Request> {
    let (requests, queue) = mpsc::channel::<Request>();
    thread::spawn(move || {
        let mut matcher = Matcher::new(Config::DEFAULT);
        let mut buf = Vec::new();
        while let Ok(mut request) = queue.recv() {
            // Queries that wait behind this one are stale: only the newest one runs.
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

/// The indices of the names that match `query`, best score first. An empty query keeps the order.
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
    use super::{FilterWorker, rank};
    use nucleo_matcher::{Config, Matcher};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    /// The ranking of one query with a new matcher, as the old thread per key press did it.
    fn filter(query: &str, filenames: &[String]) -> Vec<usize> {
        rank(
            query,
            filenames,
            &mut Matcher::new(Config::DEFAULT),
            &mut Vec::new(),
        )
    }

    /// Wait at most 2 s for the answer, so a hang fails the test.
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
    fn the_worker_answers_two_queries_with_one_copy_of_the_list() {
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
    fn each_query_has_its_own_reply_channel() {
        let filenames: Vec<String> = vec!["vacation.jpg".into(), "report.png".into()];
        let mut worker = FilterWorker::default();

        let old = worker.submit("vac", &filenames);
        let new = worker.submit("rep", &filenames);

        assert_eq!(answer(&new), vec![1]);
        assert!(new.recv_timeout(Duration::from_millis(50)).is_err());
        drop(old);
    }

    /// The time per key press of the old way (a copy of the list, a thread, a new matcher) and of
    /// the worker, on 10 000 synthetic names: `cargo test --release -- --ignored search::`.
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

        // The old way: a copy of the list, a thread and a new matcher per key press. `ui` is the
        // part on the thread of the UI (the copy and the thread start).
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
