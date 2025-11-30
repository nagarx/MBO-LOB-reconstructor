//! Benchmarks for LOB reconstruction performance.

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use mbo_lob_reconstructor::{Action, LobReconstructor, MboMessage, Side};

fn create_test_messages(count: usize) -> Vec<MboMessage> {
    let mut messages = Vec::with_capacity(count);
    let base_price: i64 = 100_000_000_000; // $100.00
    
    for i in 0..count {
        let order_id = (i + 1) as u64;
        let is_bid = i % 2 == 0;
        let price_offset = ((i % 10) as i64) * 10_000_000; // 0.01 increments
        
        let price = if is_bid {
            base_price - price_offset
        } else {
            base_price + 10_000_000 + price_offset
        };
        
        messages.push(MboMessage::new(
            order_id,
            Action::Add,
            if is_bid { Side::Bid } else { Side::Ask },
            price,
            ((i % 100) + 1) as u32,
        ));
    }
    
    messages
}

fn bench_reconstruction(c: &mut Criterion) {
    let messages = create_test_messages(10_000);
    
    let mut group = c.benchmark_group("reconstruction");
    group.throughput(Throughput::Elements(messages.len() as u64));
    
    group.bench_function("process_messages", |b| {
        b.iter(|| {
            let mut lob = LobReconstructor::new(10);
            for msg in &messages {
                let _ = black_box(lob.process_message(msg));
            }
        })
    });
    
    group.finish();
}

fn bench_analytics(c: &mut Criterion) {
    // Build a populated LOB first
    let messages = create_test_messages(100);
    let mut lob = LobReconstructor::new(10);
    let mut last_state = None;
    
    for msg in &messages {
        last_state = lob.process_message(msg).ok();
    }
    
    let state = last_state.unwrap();
    
    let mut group = c.benchmark_group("analytics");
    
    group.bench_function("microprice", |b| {
        b.iter(|| black_box(state.microprice()))
    });
    
    group.bench_function("spread_bps", |b| {
        b.iter(|| black_box(state.spread_bps()))
    });
    
    group.bench_function("depth_imbalance", |b| {
        b.iter(|| black_box(state.depth_imbalance()))
    });
    
    group.bench_function("vwap_bid", |b| {
        b.iter(|| black_box(state.vwap_bid(5)))
    });
    
    group.bench_function("check_consistency", |b| {
        b.iter(|| black_box(state.check_consistency()))
    });
    
    group.finish();
}

criterion_group!(benches, bench_reconstruction, bench_analytics);
criterion_main!(benches);

