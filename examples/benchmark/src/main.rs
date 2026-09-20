use adb_core::{Row, RowId, Value};
use adb_engine::Database;
use adb_execution::{encode_batch_v1, BinaryOp, Expr, PhysicalPlan};
use adb_ffi::{
    adb_batch_data, adb_batch_len, adb_batch_release, adb_close, adb_execute_plan_json, adb_open,
    adb_query_close, adb_query_next_batch, AdbStatus,
};
use std::{
    env, fs,
    hint::black_box,
    ptr, slice,
    time::{Duration, Instant},
};
use tempfile::tempdir;

#[derive(Clone)]
struct M {
    name: &'static str,
    ops: u64,
    elapsed: Duration,
}
impl M {
    fn rate(&self) -> f64 {
        self.ops as f64 / self.elapsed.as_secs_f64().max(1e-12)
    }
}
fn row(id: u128) -> Row {
    Row::new()
        .with_field(1, Value::Int64(id as i64))
        .with_field(2, Value::String(format!("row-{id}")))
        .with_field(3, Value::Bool(id % 2 == 0))
}
fn measure<F: FnMut()>(name: &'static str, ops: u64, mut f: F) -> M {
    let s = Instant::now();
    f();
    M {
        name,
        ops,
        elapsed: s.elapsed(),
    }
}
fn drain(db: &Database, plan: PhysicalPlan) -> usize {
    let mut c = db.execute(plan).unwrap();
    let mut n = 0;
    while let Some(b) = c.next_batch().unwrap() {
        n += b.len();
        black_box(b);
    }
    n
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut rows = 10_000usize;
    let mut iters = 100usize;
    let mut output = None;
    let a = env::args().skip(1).collect::<Vec<_>>();
    let mut i = 0;
    while i < a.len() {
        match a[i].as_str() {
            "--rows" => {
                i += 1;
                rows = a[i].parse()?
            }
            "--iters" => {
                i += 1;
                iters = a[i].parse()?
            }
            "--output" => {
                i += 1;
                output = Some(a[i].clone())
            }
            _ => {}
        }
        i += 1;
    }
    let dir = tempdir()?;
    let db = Database::open(dir.path())?;
    let mut tx = db.begin();
    for id in 1..=rows as u128 {
        tx.put(RowId(id), row(id));
    }
    db.commit(tx)?;
    let scan = PhysicalPlan::Scan;
    let filtered = PhysicalPlan::Filter {
        input: Box::new(PhysicalPlan::Scan),
        predicate: Expr::Binary {
            left: Box::new(Expr::Column { field_id: 1 }),
            op: BinaryOp::Gt,
            right: Box::new(Expr::Literal {
                value: Value::Int64((rows / 2) as i64),
            }),
        },
    };
    let projected = PhysicalPlan::Project {
        input: Box::new(filtered.clone()),
        fields: vec![1, 2],
    };
    let limited = PhysicalPlan::Limit {
        input: Box::new(projected.clone()),
        limit: 100,
    };
    let mut ms = Vec::new();
    ms.push(measure("execution_scan", rows as u64, || {
        black_box(drain(&db, scan.clone()));
    }));
    ms.push(measure("execution_filter", rows as u64, || {
        black_box(drain(&db, filtered.clone()));
    }));
    ms.push(measure("execution_project", (rows / 2) as u64, || {
        black_box(drain(&db, projected.clone()));
    }));
    ms.push(measure("execution_limit", 100, || {
        black_box(drain(&db, limited.clone()));
    }));
    ms.push(measure("point_lookup", iters as u64, || {
        for k in 0..iters {
            let id = (k % rows + 1) as u128;
            black_box(drain(&db, PhysicalPlan::PointLookup { row_id: RowId(id) }));
        }
    }));
    let mut c = db.execute(limited.clone())?;
    let b = c.next_batch()?.unwrap();
    ms.push(measure("batch_wire_encode", iters as u64, || {
        for _ in 0..iters {
            black_box(encode_batch_v1(&b).unwrap());
        }
    }));
    drop(db);
    let path = dir.path().to_string_lossy().as_bytes().to_vec();
    let mut h = ptr::null_mut();
    assert_eq!(adb_open(path.as_ptr(), path.len(), &mut h), AdbStatus::Ok);
    let plan_json = serde_json::to_vec(&limited)?;
    ms.push(measure("ffi_execute_and_batch", iters as u64, || {
        for _ in 0..iters {
            let mut q = ptr::null_mut();
            assert_eq!(
                adb_execute_plan_json(h, plan_json.as_ptr(), plan_json.len(), &mut q),
                AdbStatus::Ok
            );
            let mut bh = ptr::null_mut();
            assert_eq!(adb_query_next_batch(q, &mut bh), AdbStatus::Ok);
            let n = adb_batch_len(bh);
            let p = adb_batch_data(bh);
            black_box(unsafe { slice::from_raw_parts(p, n) });
            assert_eq!(adb_batch_release(bh), AdbStatus::Ok);
            assert_eq!(adb_query_close(q), AdbStatus::Ok);
        }
    }));
    assert_eq!(adb_close(h), AdbStatus::Ok);
    println!("Adaptive DB 1.7.2 execution/FFI benchmark; rows={rows}, iters={iters}");
    for m in &ms {
        println!(
            "{:<28} {:>10.3} ms {:>14.2} ops/s",
            m.name,
            m.elapsed.as_secs_f64() * 1000.0,
            m.rate()
        );
    }
    if let Some(path) = output {
        let body=format!("{{\n  \"milestone\": \"1.7.2\",\n  \"engine_base\": \"1.7.1-hardened\",\n  \"rows\": {rows},\n  \"iters\": {iters},\n  \"measurements\": [\n{}\n  ]\n}}\n",ms.iter().map(|m|format!("    {{\"name\":\"{}\",\"ops\":{},\"elapsed_ns\":{},\"ops_per_sec\":{:.3}}}",m.name,m.ops,m.elapsed.as_nanos(),m.rate())).collect::<Vec<_>>().join(",\n"));
        fs::write(path, body)?;
    }
    Ok(())
}
