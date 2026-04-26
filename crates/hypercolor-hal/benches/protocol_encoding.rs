use std::hint::black_box;
use std::time::Duration;

use criterion::{
    BenchmarkGroup, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main,
    measurement::WallTime,
};
use hypercolor_hal::drivers::nollie::{NollieModel, NollieProtocol, ProtocolVersion};
use hypercolor_hal::drivers::qmk::{ProtocolRevision, QmkKeyboardConfig, QmkProtocol};
use hypercolor_hal::protocol::Protocol;

fn benchmark_config() -> Criterion {
    Criterion::default()
        .warm_up_time(Duration::from_millis(500))
        .measurement_time(Duration::from_secs(2))
        .sample_size(50)
}

fn gradient_frame(led_count: usize) -> Vec<[u8; 3]> {
    let span = led_count.saturating_sub(1).max(1);

    (0..led_count)
        .map(|index| {
            let red = u8::try_from((index * 255) / span).expect("red channel fits in u8");
            let green = 255_u8.saturating_sub(red);
            let blue = u8::try_from(((index * 127) / span) + 32).expect("blue channel fits in u8");
            [red, green, blue]
        })
        .collect()
}

fn bench_protocol_case<P>(
    group: &mut BenchmarkGroup<'_, WallTime>,
    bench_name: &str,
    led_count: usize,
    protocol: &P,
) where
    P: Protocol,
{
    let colors = gradient_frame(led_count);
    let mut commands = Vec::new();

    group.throughput(Throughput::Elements(
        u64::try_from(led_count).expect("LED count fits in u64"),
    ));
    group.bench_function(BenchmarkId::from_parameter(bench_name), |b| {
        b.iter(|| {
            protocol.encode_frame_into(black_box(&colors), &mut commands);
            black_box(&commands);
        });
    });
}

fn bench_protocol_encoding(c: &mut Criterion) {
    let mut group = c.benchmark_group("hal_protocol_encoding");
    let qmk_revd_87 = QmkProtocol::new(QmkKeyboardConfig::new(87, ProtocolRevision::RevD));
    let qmk_revd_104 = QmkProtocol::new(QmkKeyboardConfig::new(104, ProtocolRevision::RevD));
    let prism8 = NollieProtocol::new(NollieModel::Prism8);
    let nollie16v3 = NollieProtocol::new(NollieModel::Nollie16v3);
    let nollie32 = NollieProtocol::new(NollieModel::Nollie32 {
        protocol_version: ProtocolVersion::V2,
    });

    bench_protocol_case(&mut group, "qmk_revd_87", 87, &qmk_revd_87);
    bench_protocol_case(&mut group, "qmk_revd_104", 104, &qmk_revd_104);
    bench_protocol_case(&mut group, "prism8_1008", 1_008, &prism8);
    bench_protocol_case(&mut group, "nollie16v3_4096", 4_096, &nollie16v3);
    bench_protocol_case(&mut group, "nollie32_5120", 5_120, &nollie32);

    group.finish();
}

criterion_group! {
    name = benches;
    config = benchmark_config();
    targets = bench_protocol_encoding
}
criterion_main!(benches);
