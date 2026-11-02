use super::*;

const GIB: u64 = 1 << 20;

/// The host the operator saw `status` time out on: load 45 on 12 cores,
/// 12 GB of swap on 15 GB of RAM, a quarter of the time stalled on disk.
fn overloaded() -> Readings {
    Readings {
        load: Some(45.0),
        cores: 12,
        memory: Some(Memory {
            total_kib: 15 * GIB,
            available_kib: GIB,
            swap_used_kib: 12 * GIB,
        }),
        io_stall: Some(25.0),
    }
}

#[test]
fn an_overloaded_host_names_each_sign_it_shows() {
    let said = describe(&overloaded()).expect("overloaded");
    assert_eq!(
        said,
        "this host is overloaded: load 45.0 on 12 cores, 12.0 GiB of swap in use and \
         1.0 GiB of memory available, tasks waited on the disk 25% of the last 10 s"
    );
}

#[test]
fn a_quiet_or_unreadable_host_names_nothing() {
    let quiet = Readings {
        load: Some(3.0),
        cores: 12,
        memory: Some(Memory {
            total_kib: 15 * GIB,
            available_kib: 8 * GIB,
            swap_used_kib: GIB,
        }),
        io_stall: Some(2.0),
    };
    assert_eq!(describe(&quiet), None);
    let unreadable = Readings {
        cores: 4,
        ..Readings::default()
    };
    assert_eq!(describe(&unreadable), None);
    let only_disk = Readings {
        io_stall: Some(40.0),
        ..quiet
    };
    assert_eq!(
        describe(&only_disk).as_deref(),
        Some("this host is overloaded: tasks waited on the disk 40% of the last 10 s")
    );
}

#[test]
fn proc_files_parse_and_garbage_does_not() {
    assert_eq!(parse_load("45.12 30.01 20.00 3/2048 12345\n"), Some(45.12));
    assert_eq!(parse_load("not a number"), None);
    assert_eq!(parse_load(""), None);

    let meminfo = "MemTotal:       15728640 kB\nMemFree:          100000 kB\n\
                   MemAvailable:    1048576 kB\nSwapTotal:      28311552 kB\n\
                   SwapFree:       15728640 kB\n";
    assert_eq!(
        parse_memory(meminfo),
        Some(Memory {
            total_kib: 15 * GIB,
            available_kib: GIB,
            swap_used_kib: 12 * GIB,
        })
    );
    assert_eq!(parse_memory("MemTotal: 10 kB\n"), None, "fields missing");
    let impossible = "MemTotal: 10 kB\nMemAvailable: 20 kB\nSwapTotal: 0 kB\nSwapFree: 0 kB\n";
    assert_eq!(parse_memory(impossible), None);

    let io = "some avg10=24.73 avg60=20.10 avg300=12.00 total=123456\n\
              full avg10=10.00 avg60=8.00 avg300=4.00 total=65432\n";
    assert_eq!(parse_stall(io), Some(24.73));
    assert_eq!(parse_stall("full avg10=10.00\n"), None);
    assert_eq!(parse_stall("some avg10=400.0\n"), None);
}
