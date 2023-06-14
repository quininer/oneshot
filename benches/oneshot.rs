use criterion::{ criterion_main, criterion_group, Criterion, black_box };
use tokio::runtime;


fn bench_oneshot_channel(c: &mut Criterion) {
    let rt = runtime::Builder::new_current_thread()
        .build()
        .unwrap();

    c.bench_function("oneshot", |b| {
        use futures_oneshot as oneshot;

        b.iter(|| {
            let (tx, rx) = oneshot::channel::<usize>();
            tx.send(black_box(0x42)).unwrap();
            rt.block_on(rx).unwrap();
        });
    });

    c.bench_function("futures-channel", |b| {
        use futures_channel::oneshot::channel;

        b.iter(|| {
            let (tx, rx) = channel::<usize>();
            tx.send(black_box(0x42)).unwrap();
            rt.block_on(rx).unwrap();
        })
    });

    c.bench_function("tokio::sync", |b| {
        use tokio::sync::oneshot::channel;

        b.iter(|| {
            let (tx, rx) = channel::<usize>();
            tx.send(black_box(0x42)).unwrap();
            rt.block_on(rx).unwrap();
        })
    });
}


criterion_group!(oneshot, bench_oneshot_channel);
criterion_main!(oneshot);
