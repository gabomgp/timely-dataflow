extern crate time;
extern crate rand;
extern crate timely;
extern crate timely_sort;

use std::collections::HashMap;
use rand::{Rng, SeedableRng, StdRng};

use timely::dataflow::{InputHandle, ProbeHandle};

use timely::dataflow::operators::{LoopVariable, ConnectLoop, Probe};
use timely::dataflow::operators::generic::Operator;
use timely::dataflow::channels::pact::Exchange;

fn main() {

    timely::execute_from_args(std::env::args().skip(3), move |worker| {

        let mut input = InputHandle::new();
        let mut probe = ProbeHandle::new();

        worker.dataflow(|scope| {

            // create a new input, into which we can push edge changes.
            let edge_stream = input.to_stream(scope);

            // create a new feedback stream, which will be changes to ranks.
            let (handle, rank_stream) = scope.loop_variable(usize::max_value(), 1);

            // bring edges and ranks together!
            let changes = edge_stream.binary_frontier(
                &rank_stream, 
                Exchange::new(|x: &((usize, usize), i64)| (x.0).0 as u64),
                Exchange::new(|x: &(usize, i64)| x.0 as u64),
                "PageRank",
                |_capability| {

                    // where we stash out-of-order data.
                    let mut edge_stash = HashMap::new();
                    let mut rank_stash = HashMap::new();

                    // lists of edges, ranks, and changes.
                    let mut edges = Vec::new();
                    let mut ranks = Vec::new();
                    let mut delta = Vec::new();

                    let timer = ::std::time::Instant::now();
                    
                    move |input1, input2, output| {

                        // hold on to edge changes until it is time.
                        input1.for_each(|time, data| {
                            edge_stash.entry(time).or_insert(vec![]).extend(data.drain(..));
                        });

                        // hold on to rank changes until it is time.
                        input2.for_each(|time, data| {
                            rank_stash.entry(time).or_insert(vec![]).extend(data.drain(..));
                        });

                        let frontiers = &[input1.frontier(), input2.frontier()];

                        for (time, edge_changes) in edge_stash.iter_mut() {
                            if frontiers.iter().all(|f| !f.less_equal(time)) {
                                
                                let mut session = output.session(&time);
            
                                compact(edge_changes);

                                for ((src, dst), diff) in edge_changes.drain(..) {

                                    // 0. ensure enough state allocated
                                    while edges.len() <= src { edges.push(Vec::new()); }
                                    while ranks.len() <= src { ranks.push(1_000); }

                                    // 1. subtract previous distribution.
                                    allocate(ranks[src], &edges[src][..], &mut delta);
                                    for x in delta.iter_mut() { x.1 *= -1; }

                                    // 2. update edges.
                                    edges[src].push((dst, diff));
                                    compact(&mut edges[src]);

                                    // 3. re-distribute allocations.
                                    allocate(ranks[src], &edges[src][..], &mut delta);

                                    // 4. compact down and send cumulative changes.
                                    compact(&mut delta);
                                    for (dst, diff) in delta.drain(..) {
                                        session.give((dst, diff));
                                    }
                                }
                            }
                        }

                        edge_stash.retain(|_key, val| val.len() > 0);

                        for (time, rank_changes) in rank_stash.iter_mut() {
                            if frontiers.iter().all(|f| !f.less_equal(time)) {
                                
                                let mut session = output.session(&time);

                                compact(rank_changes);

                                let mut cnt = 0;
                                let mut sum = 0;
                                let mut max = 0;

                                for (src, diff) in rank_changes.drain(..) {

                                    cnt += 1;
                                    sum += diff.abs();
                                    max = if max < diff.abs() { diff.abs() } else { max };

                                    // 0. ensure enough state allocated
                                    while edges.len() <= src { edges.push(Vec::new()); }
                                    while ranks.len() <= src { ranks.push(1_000); }

                                    // 1. subtract previous distribution.
                                    allocate(ranks[src], &edges[src][..], &mut delta);
                                    for x in delta.iter_mut() { x.1 *= -1; }

                                    // 2. update ranks.
                                    ranks[src] += diff;

                                    // 3. re-distribute allocations.
                                    allocate(ranks[src], &edges[src][..], &mut delta);

                                    // 4. compact down and send cumulative changes.
                                    compact(&mut delta);
                                    for (dst, diff) in delta.drain(..) {
                                        session.give((dst, diff));
                                    }
                                }

                                println!("{:?}:\t{:?}\t{}\t{}\t{}", timer.elapsed(), time.time(), cnt, sum, max);
                            }
                        }

                        rank_stash.retain(|_key, val| val.len() > 0);
                        
                    }
                }
            );

            changes
                .probe_with(&mut probe)
                .connect_loop(handle);

        });

        let nodes: usize = std::env::args().nth(1).unwrap().parse().unwrap();
        let edges: usize = std::env::args().nth(2).unwrap().parse().unwrap();

        let seed: &[_] = &[1, 2, 3, worker.index()];
        let mut rng1: StdRng = SeedableRng::from_seed(seed);
        let mut rng2: StdRng = SeedableRng::from_seed(seed);

        for _ in 0 .. edges / worker.peers() {
            input.send(((rng1.gen_range(0, nodes), rng1.gen_range(0, nodes)), 1));
        }

        input.advance_to(1);

        while probe.less_than(input.time()) {
            worker.step();
        }

        for i in 1 .. 1000 {
            input.send(((rng1.gen_range(0, nodes), rng1.gen_range(0, nodes)), 1));
            input.send(((rng2.gen_range(0, nodes), rng2.gen_range(0, nodes)), -1));
            input.advance_to(1 + i);
            while probe.less_than(input.time()) {
                worker.step();
            }
        }

    }).unwrap(); // asserts error-free execution;
}

fn compact<T: Ord>(list: &mut Vec<(T, i64)>) {
    if list.len() > 0 {
        list.sort_by(|x,y| x.0.cmp(&y.0));
        for i in 0 .. list.len() - 1 {
            if list[i].0 == list[i+1].0 {
                list[i+1].1 += list[i].1;
                list[i].1 = 0;
            }
        }
        list.retain(|x| x.1 != 0);
    } 
}

// this method allocates some rank between elements of `edges`.
fn allocate(rank: i64, edges: &[(usize, i64)], send: &mut Vec<(usize, i64)>) {
    if edges.len() > 0 {
        assert!(rank >= 0);
        assert!(edges.iter().all(|x| x.1 > 0));

        let degree: i64 = edges.iter().map(|x| x.1 as i64).sum();
        let share = ((rank * 5) / 6) / degree;
        for i in 0 .. edges.len() {
            if (i as i64) < (share % (edges.len() as i64)) {
                send.push((edges[i].0, edges[i].1 * (share + 1)));
            }
            else {
                send.push((edges[i].0, edges[i].1 * share));
            }
        }
    }
}