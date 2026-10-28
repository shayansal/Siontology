//! Seeded fault injection between nodes: loss, delay with jitter,
//! duplication, reordering and partitions, per message class.
//!
//! A [`FaultNet`] holds the faults in force for one network, and every
//! node's transport is wrapped in a [`FaultyTransport`] that consults it on
//! each send. Decisions come from one seeded generator per link (sender,
//! receiver), so they depend only on the seed and on the order in which a
//! sender sends on that link, never on how threads interleave: a failure
//! reproduces from its seed as long as the protocol sends the same
//! sequence. A timer thread releases delayed messages. A [`FaultScript`]
//! changes the faults over time.
//!
//! At zero faults a `FaultyTransport` hands every message straight to the
//! transport it wraps, so it behaves exactly as that transport does.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};
use std::fmt;
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rand_chacha::ChaCha20Rng;
use rand_core::RngCore;
use serde::{Deserialize, Serialize};

use super::{MessageClass, NodeId, PeerMessage, TrafficCounter, TrafficStats, Transport};

/// The faults of one message class. Rates are in parts per million.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassFaults {
    /// Messages lost.
    pub loss_ppm: u32,
    /// Messages delivered twice.
    pub duplicate_ppm: u32,
    /// Messages held back long enough for later ones to overtake them.
    pub reorder_ppm: u32,
    /// One-way delay, in milliseconds.
    pub delay_ms: u64,
    /// Extra delay, uniform between zero and this, in milliseconds.
    pub jitter_ms: u64,
}

impl ClassFaults {
    /// Whether these faults change nothing.
    pub fn is_none(&self) -> bool {
        *self == ClassFaults::default()
    }

    /// Losing `percent` of messages, nothing else.
    pub fn loss_percent(percent: u32) -> ClassFaults {
        ClassFaults {
            loss_ppm: percent.min(100) * 10_000,
            ..ClassFaults::default()
        }
    }
}

/// The faults in force.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Faults {
    /// Faults per class; a class not listed has none.
    pub classes: BTreeMap<MessageClass, ClassFaults>,
    /// Groups of nodes cut off from each other: a message between nodes of
    /// two different groups is lost. A node in no group reaches everyone.
    pub partition: Vec<BTreeSet<NodeId>>,
}

impl Faults {
    /// No faults.
    pub fn none() -> Faults {
        Faults::default()
    }

    /// The same faults for every class.
    pub fn uniform(faults: ClassFaults) -> Faults {
        Faults {
            classes: MessageClass::ALL.iter().map(|c| (*c, faults)).collect(),
            partition: Vec::new(),
        }
    }

    /// Whether these faults change nothing.
    pub fn is_none(&self) -> bool {
        self.partition.is_empty() && self.classes.values().all(ClassFaults::is_none)
    }

    /// Whether a partition cuts `a` off from `b`.
    pub fn separates(&self, a: &NodeId, b: &NodeId) -> bool {
        let group = |node: &NodeId| self.partition.iter().position(|g| g.contains(node));
        matches!((group(a), group(b)), (Some(x), Some(y)) if x != y)
    }
}

/// Faults that change over time: each step's faults take effect at its
/// offset from the start of the script.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FaultScript {
    /// (offset, faults) in order of offset.
    pub steps: Vec<(Duration, Faults)>,
}

/// What the injector did, for every node of its network.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Injected {
    /// Messages lost to the loss rate.
    pub lost: u64,
    /// Messages lost to a partition.
    pub partitioned: u64,
    /// Extra copies delivered.
    pub duplicated: u64,
    /// Messages delayed.
    pub delayed: u64,
    /// Messages held back to be overtaken.
    pub reordered: u64,
}

type Job = Box<dyn FnOnce() + Send>;

#[derive(Default)]
struct TimerQueue {
    heap: BinaryHeap<Reverse<(Instant, u64)>>,
    jobs: BTreeMap<u64, Job>,
    next: u64,
    stop: bool,
}

/// Runs jobs at their time on one thread.
#[derive(Default)]
struct Timer {
    queue: Mutex<TimerQueue>,
    wake: Condvar,
}

impl Timer {
    fn start() -> (Arc<Timer>, JoinHandle<()>) {
        let timer = Arc::new(Timer::default());
        let worker = timer.clone();
        let handle = thread::Builder::new()
            .name("fault-timer".into())
            .spawn(move || worker.run())
            .expect("spawn the fault timer");
        (timer, handle)
    }

    fn after(&self, delay: Duration, job: Job) {
        let Ok(mut queue) = self.queue.lock() else {
            return;
        };
        let seq = queue.next;
        queue.next += 1;
        queue.heap.push(Reverse((Instant::now() + delay, seq)));
        queue.jobs.insert(seq, job);
        self.wake.notify_one();
    }

    fn run(&self) {
        let Ok(mut queue) = self.queue.lock() else {
            return;
        };
        loop {
            if queue.stop {
                return;
            }
            let now = Instant::now();
            match queue.heap.peek().copied() {
                Some(Reverse((at, seq))) if at <= now => {
                    queue.heap.pop();
                    if let Some(job) = queue.jobs.remove(&seq) {
                        // Run without the lock: the job sends, and a send
                        // may schedule again.
                        drop(queue);
                        job();
                        queue = match self.queue.lock() {
                            Ok(queue) => queue,
                            Err(_) => return,
                        };
                    }
                }
                Some(Reverse((at, _))) => {
                    queue = match self.wake.wait_timeout(queue, at - now) {
                        Ok((queue, _)) => queue,
                        Err(_) => return,
                    };
                }
                None => {
                    queue = match self.wake.wait(queue) {
                        Ok(queue) => queue,
                        Err(_) => return,
                    };
                }
            }
        }
    }

    fn stop(&self) {
        if let Ok(mut queue) = self.queue.lock() {
            queue.stop = true;
            queue.jobs.clear();
        }
        self.wake.notify_all();
    }
}

/// What to do with one message.
enum Decision {
    /// No faults apply: hand it on now.
    Pass,
    /// It is lost.
    Lose,
    /// Deliver a copy after each delay.
    Deliver(Vec<Duration>),
}

/// The faults of one network, shared by the transports of all its nodes.
pub struct FaultNet {
    seed: u64,
    faults: RwLock<Faults>,
    links: Mutex<BTreeMap<(NodeId, NodeId), ChaCha20Rng>>,
    injected: Mutex<Injected>,
    timer: Arc<Timer>,
    timer_thread: Mutex<Option<JoinHandle<()>>>,
}

impl fmt::Debug for FaultNet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FaultNet")
            .field("seed", &self.seed)
            .field("faults", &self.faults())
            .finish()
    }
}

impl Drop for FaultNet {
    fn drop(&mut self) {
        self.timer.stop();
        if let Ok(mut thread) = self.timer_thread.lock() {
            if let Some(thread) = thread.take() {
                let _ = thread.join();
            }
        }
    }
}

/// Draws a rate in parts per million.
fn roll(rng: &mut ChaCha20Rng, ppm: u32) -> bool {
    ppm > 0 && rng.next_u32() % 1_000_000 < ppm
}

/// Draws a uniform value in `0..=max`.
fn upto(rng: &mut ChaCha20Rng, max: u64) -> u64 {
    if max == 0 {
        0
    } else {
        rng.next_u64() % (max + 1)
    }
}

impl FaultNet {
    /// A network injecting `faults`, with decisions drawn from `seed`.
    pub fn new(seed: u64, faults: Faults) -> Arc<FaultNet> {
        let (timer, thread) = Timer::start();
        Arc::new(FaultNet {
            seed,
            faults: RwLock::new(faults),
            links: Mutex::new(BTreeMap::new()),
            injected: Mutex::new(Injected::default()),
            timer,
            timer_thread: Mutex::new(Some(thread)),
        })
    }

    /// Replaces the faults in force.
    pub fn set(&self, faults: Faults) {
        if let Ok(mut current) = self.faults.write() {
            *current = faults;
        }
    }

    /// The faults in force.
    pub fn faults(&self) -> Faults {
        self.faults.read().map(|f| f.clone()).unwrap_or_default()
    }

    /// What the injector has done so far.
    pub fn injected(&self) -> Injected {
        self.injected.lock().map(|i| *i).unwrap_or_default()
    }

    /// Applies `script` from now on, on its own thread.
    pub fn run_script(self: &Arc<Self>, script: FaultScript) -> JoinHandle<()> {
        let net = Arc::downgrade(self);
        thread::spawn(move || {
            let start = Instant::now();
            for (offset, faults) in script.steps {
                if let Some(wait) = (start + offset).checked_duration_since(Instant::now()) {
                    thread::sleep(wait);
                }
                match net.upgrade() {
                    Some(net) => net.set(faults),
                    None => return,
                }
            }
        })
    }

    /// Wraps a node's transport.
    pub fn wrap<T: Transport>(self: &Arc<Self>, inner: T) -> FaultyTransport<T> {
        FaultyTransport {
            inner: Arc::new(inner),
            net: self.clone(),
            drops: Arc::new(TrafficCounter::default()),
        }
    }

    fn count(&self, f: impl FnOnce(&mut Injected)) {
        if let Ok(mut injected) = self.injected.lock() {
            f(&mut injected);
        }
    }

    fn decide(&self, from: NodeId, to: NodeId, class: MessageClass) -> Decision {
        let Ok(faults) = self.faults.read() else {
            return Decision::Pass;
        };
        if faults.separates(&from, &to) {
            self.count(|i| i.partitioned += 1);
            return Decision::Lose;
        }
        let Some(class_faults) = faults.classes.get(&class).copied() else {
            return Decision::Pass;
        };
        drop(faults);
        if class_faults.is_none() {
            return Decision::Pass;
        }
        let Ok(mut links) = self.links.lock() else {
            return Decision::Pass;
        };
        let rng = links.entry((from, to)).or_insert_with(|| {
            sion_core::rng::labeled(self.seed, &format!("sion/node/fault-link/{from}/{to}"))
        });
        if roll(rng, class_faults.loss_ppm) {
            drop(links);
            self.count(|i| i.lost += 1);
            return Decision::Lose;
        }
        let mut delay = class_faults.delay_ms + upto(rng, class_faults.jitter_ms);
        let reordered = roll(rng, class_faults.reorder_ppm);
        if reordered {
            // Held back past the delay and jitter of the messages behind it.
            delay += class_faults.delay_ms + class_faults.jitter_ms + 1 + upto(rng, 50);
        }
        let mut copies = vec![Duration::from_millis(delay)];
        let duplicated = roll(rng, class_faults.duplicate_ppm);
        if duplicated {
            copies.push(Duration::from_millis(
                delay + upto(rng, class_faults.jitter_ms),
            ));
        }
        drop(links);
        self.count(|i| {
            i.reordered += u64::from(reordered);
            i.duplicated += u64::from(duplicated);
            i.delayed += u64::from(delay > 0);
        });
        Decision::Deliver(copies)
    }
}

/// A transport whose messages pass through a [`FaultNet`] first.
pub struct FaultyTransport<T> {
    inner: Arc<T>,
    net: Arc<FaultNet>,
    /// Messages the injector lost, by class, kind and peer.
    drops: Arc<TrafficCounter>,
}

impl<T: Transport> fmt::Debug for FaultyTransport<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FaultyTransport({:?})", self.inner)
    }
}

impl<T: Transport + 'static> Transport for FaultyTransport<T> {
    fn local(&self) -> NodeId {
        self.inner.local()
    }

    fn send(&self, to: NodeId, message: PeerMessage) {
        let class = message.class();
        match self.net.decide(self.inner.local(), to, class) {
            Decision::Pass => self.inner.send(to, message),
            Decision::Lose => self.drops.dropped(to, class, message.kind()),
            Decision::Deliver(copies) => {
                for delay in copies {
                    if delay.is_zero() {
                        self.inner.send(to, message.clone());
                    } else {
                        let inner = self.inner.clone();
                        let message = message.clone();
                        self.net
                            .timer
                            .after(delay, Box::new(move || inner.send(to, message)));
                    }
                }
            }
        }
    }

    fn knows(&self, node: &NodeId) -> bool {
        self.inner.knows(node)
    }

    fn set_peers(&self, peers: &super::PeerSet) {
        self.inner.set_peers(peers);
    }

    fn stats(&self) -> TrafficStats {
        let mut stats = self.inner.stats();
        let drops = self.drops.snapshot("");
        stats.transport = format!("{}+faults", stats.transport);
        for (map, extra) in [
            (&mut stats.classes, &drops.classes),
            (&mut stats.kinds, &drops.kinds),
            (&mut stats.peers, &drops.peers),
        ] {
            for (key, traffic) in extra {
                map.entry(key.clone()).or_default().dropped += traffic.dropped;
            }
        }
        stats
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use sion_core::ids::{Pulse, ValidatorId};

    use super::super::memory::MemoryNetwork;
    use super::super::ClassLimits;
    use super::*;

    type Inbox = Arc<Mutex<Vec<(NodeId, PeerMessage)>>>;

    fn node(name: &[u8]) -> NodeId {
        NodeId::Validator(ValidatorId::derive(name))
    }

    fn message(n: u64) -> PeerMessage {
        PeerMessage::PulseSignatureRequest { pulse: Pulse(n) }
    }

    /// Nodes `a`, `b` and `c` on a memory network behind `net`.
    fn nodes(
        net: &Arc<FaultNet>,
    ) -> Vec<(
        FaultyTransport<super::super::memory::MemoryTransport>,
        Inbox,
    )> {
        let memory = MemoryNetwork::new(ClassLimits::default(), false);
        [b"a", b"b", b"c"]
            .iter()
            .map(|name| {
                let inbox: Inbox = Arc::default();
                let sink = inbox.clone();
                let endpoint = memory.endpoint(
                    node(*name),
                    Arc::new(move |from, m| sink.lock().unwrap().push((from, m))),
                );
                (net.wrap(endpoint), inbox)
            })
            .collect()
    }

    fn wait_for(inbox: &Inbox, count: usize) -> Vec<(NodeId, PeerMessage)> {
        let deadline = Instant::now() + Duration::from_secs(10);
        while inbox.lock().unwrap().len() < count && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        inbox.lock().unwrap().clone()
    }

    #[test]
    fn at_zero_faults_it_delivers_exactly_what_the_memory_transport_does() {
        let net = FaultNet::new(1, Faults::none());
        let nodes = nodes(&net);
        let (a, b) = (&nodes[0].0, &nodes[1].0);
        for n in 0..50 {
            a.send(b.local(), message(n));
        }
        let expected: Vec<_> = (0..50).map(|n| (a.local(), message(n))).collect();
        // Synchronous, in order, exactly once.
        assert_eq!(*nodes[1].1.lock().unwrap(), expected);
        assert_eq!(a.stats().transport, "memory+faults");
        assert_eq!(a.stats().dropped(), 0);
        assert_eq!(net.injected(), Injected::default());
    }

    #[test]
    fn loss_is_seeded_per_link_and_counted() {
        let run = |seed| {
            let net = FaultNet::new(seed, Faults::uniform(ClassFaults::loss_percent(30)));
            let nodes = nodes(&net);
            for n in 0..1_000 {
                nodes[0].0.send(nodes[1].0.local(), message(n));
            }
            let delivered: Vec<u64> = nodes[1]
                .1
                .lock()
                .unwrap()
                .iter()
                .map(|(_, m)| match m {
                    PeerMessage::PulseSignatureRequest { pulse } => pulse.0,
                    _ => unreachable!(),
                })
                .collect();
            let dropped = nodes[0].0.stats().classes["control"].dropped;
            (delivered, dropped, net.injected().lost)
        };
        let (first, dropped, lost) = run(7);
        assert!(
            (600..=800).contains(&first.len()),
            "{} delivered",
            first.len()
        );
        assert_eq!(dropped as usize, 1_000 - first.len());
        assert_eq!(lost, dropped);
        // The same seed loses the same messages; another seed others.
        assert_eq!(run(7).0, first);
        assert_ne!(run(8).0, first);
    }

    #[test]
    fn a_partition_cuts_groups_off_from_each_other_only() {
        let net = FaultNet::new(1, Faults::none());
        let nodes = nodes(&net);
        let (a, b, c) = (&nodes[0].0, &nodes[1].0, &nodes[2].0);
        net.set(Faults {
            partition: vec![[a.local()].into(), [b.local()].into()],
            ..Faults::none()
        });
        a.send(b.local(), message(1));
        a.send(c.local(), message(2));
        c.send(b.local(), message(3));
        assert_eq!(*nodes[1].1.lock().unwrap(), vec![(c.local(), message(3))]);
        assert_eq!(*nodes[2].1.lock().unwrap(), vec![(a.local(), message(2))]);
        assert_eq!(net.injected().partitioned, 1);
        // Healing restores the link.
        net.set(Faults::none());
        a.send(b.local(), message(4));
        assert_eq!(nodes[1].1.lock().unwrap().len(), 2);
    }

    #[test]
    fn delays_duplicates_and_reordering_still_deliver_everything() {
        let faults = ClassFaults {
            duplicate_ppm: 1_000_000,
            reorder_ppm: 300_000,
            delay_ms: 20,
            jitter_ms: 10,
            ..ClassFaults::default()
        };
        let net = FaultNet::new(3, Faults::uniform(faults));
        let nodes = nodes(&net);
        let (a, b) = (&nodes[0].0, &nodes[1].0);
        let start = Instant::now();
        for n in 0..40 {
            a.send(b.local(), message(n));
        }
        assert!(
            nodes[1].1.lock().unwrap().is_empty(),
            "nothing arrives at once"
        );
        let got = wait_for(&nodes[1].1, 80);
        assert!(start.elapsed() >= Duration::from_millis(20));
        assert_eq!(got.len(), 80, "every message twice");
        let order: Vec<u64> = got
            .iter()
            .map(|(_, m)| match m {
                PeerMessage::PulseSignatureRequest { pulse } => pulse.0,
                _ => unreachable!(),
            })
            .collect();
        let mut sorted = order.clone();
        sorted.sort();
        assert_ne!(order, sorted, "some messages were overtaken");
        let expected: Vec<u64> = (0..40).flat_map(|n| [n, n]).collect();
        assert_eq!(sorted, expected);
        let injected = net.injected();
        assert_eq!(injected.duplicated, 40);
        assert!(injected.reordered > 0 && injected.delayed == 40);
    }

    #[test]
    fn a_script_changes_the_faults_over_time() {
        let net = FaultNet::new(1, Faults::uniform(ClassFaults::loss_percent(100)));
        let nodes = nodes(&net);
        let (a, b) = (&nodes[0].0, &nodes[1].0);
        let script = net.run_script(FaultScript {
            steps: vec![(Duration::from_millis(100), Faults::none())],
        });
        a.send(b.local(), message(1));
        script.join().unwrap();
        a.send(b.local(), message(2));
        assert_eq!(*nodes[1].1.lock().unwrap(), vec![(a.local(), message(2))]);
        assert_eq!(a.stats().kinds["pulse_signature_request"].dropped, 1);
    }
}
