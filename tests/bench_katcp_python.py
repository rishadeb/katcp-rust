#!/usr/bin/env python
"""
Performance benchmarks for katcp-python, directly comparable with katcp-rust.

Uses the async API (CallbackClient + future_request) rather than
blocking_request, for a fair comparison with aiokatcp and katcp-rust
which both use native async clients.

Both suites use:
  - Identical iteration counts and sensor counts
  - Idiomatic API usage for each language
  - Same warmup procedures
  - Latency percentiles (p50/p95/p99) for request tests
  - Fixed-N push + drain for delivery tests (no artificial throttling)

Run with:
    source bench_venv/bin/activate
    python tests/bench_katcp_python.py
"""
from __future__ import absolute_import, division, print_function

import json
import logging
import sys
import threading
import time

# Suppress noisy katcp logging during benchmarks
logging.basicConfig(level=logging.CRITICAL)

import katcp
import tornado.gen
from katcp import CallbackClient, DeviceServer, Message, Sensor

# ---------------------------------------------------------------------------
# Constants — must match the Rust suite exactly
# ---------------------------------------------------------------------------

WARMUP = 100

REQ_ITERATIONS = 2000
HELP_ITERATIONS = 1000
CONCURRENT_CLIENTS = 10
CONCURRENT_REQS_PER_CLIENT = 500

DELIVERY_UPDATES = 10000
MULTI_SENSOR_COUNT = 50
MULTI_SENSOR_UPDATES_PER = 200  # 50 * 200 = 10K total

HIGH_SENSOR_COUNT = 1000
HIGH_SENSOR_LIST_ITERS = 50
HIGH_SENSOR_VALUE_ALL_ITERS = 50
HIGH_SENSOR_VALUE_SINGLE_ITERS = 1000
HIGH_SENSOR_REGEX_ITERS = 200

SCALE_SENSOR_COUNT = 20000
SCALE_ROUNDS = 3


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

class BenchServer(DeviceServer):
    VERSION_INFO = ("bench-server", 1, 0)
    BUILD_INFO = ("bench-build", 1, 0, "bench")

    def __init__(self, host, port, num_sensors=0):
        """Initialize bench server."""
        self._num_sensors = num_sensors
        super(BenchServer, self).__init__(host, port)

    def setup_sensors(self):
        """Setup bench sensors."""
        for i in range(self._num_sensors):
            self.add_sensor(
                Sensor.integer(
                    "sensor-{}".format(i),
                    "bench sensor",
                    "count",
                    params=[-2**62, 2**62],
                )
            )


def connect_client(port, timeout=10.0):
    """Connect an async client to the server."""
    client = CallbackClient("127.0.0.1", port, timeout=timeout)
    client.start(timeout=timeout)
    client.wait_connected(timeout=timeout)
    time.sleep(0.1)  # protocol negotiation
    return client


def run_on_ioloop(client, coro_func):
    """Run a tornado coroutine on the client's ioloop and wait for it to finish.

    Returns whatever the coroutine stores in result[0].
    """
    result = [None]
    error = [None]
    done = threading.Event()

    @tornado.gen.coroutine
    def wrapper():
        try:
            result[0] = yield coro_func()
        except Exception as e:
            error[0] = e
        finally:
            done.set()

    client.ioloop.add_callback(wrapper)
    done.wait(timeout=600)
    if error[0]:
        raise error[0]
    return result[0]


def format_rate(count, elapsed):
    """Format a rate into a string."""
    rate = count / elapsed if elapsed > 0 else 0
    if rate >= 1e6:
        return "{:.2f}M/s".format(rate / 1e6)
    elif rate >= 1e3:
        return "{:.2f}K/s".format(rate / 1e3)
    else:
        return "{:.2f}/s".format(rate)


def percentiles(latencies):
    """Calculate latencies percentiles."""
    latencies.sort()
    n = len(latencies)
    p50 = latencies[n // 2]
    p95 = latencies[int(n * 0.95)]
    p99 = latencies[int(n * 0.99)]
    return p50, p95, p99


def print_latency_stats(name, iterations, elapsed, latencies):
    """Print latency statistics."""
    p50, p95, p99 = percentiles(latencies)
    print()
    print("=== {} ===".format(name))
    print("  {} requests in {:.3f}s = {}".format(
        iterations, elapsed, format_rate(iterations, elapsed)))
    print("  Latency: p50={:.0f}us  p95={:.0f}us  p99={:.0f}us".format(
        p50, p95, p99))


results = {}


def record(name, **kwargs):
    """Record metrics into results dictionary."""
    results[name] = kwargs


# ---------------------------------------------------------------------------
# 1. Request latency: ?watchdog
# ---------------------------------------------------------------------------

def bench_01_watchdog_latency():
    """Benchmark watchdog request latency."""
    server = BenchServer("127.0.0.1", 0, num_sensors=0)
    server.start(timeout=5)
    port = server.bind_address[1]
    client = connect_client(port)

    @tornado.gen.coroutine
    def coro():
        for _ in range(WARMUP):
            yield client.future_request(Message.request("watchdog"), timeout=5)

        latencies = []
        start = time.time()
        for _ in range(REQ_ITERATIONS):
            t = time.time()
            reply, informs = yield client.future_request(
                Message.request("watchdog"), timeout=5)
            latencies.append((time.time() - t) * 1e6)
            assert reply.reply_ok(), reply
        elapsed = time.time() - start
        raise tornado.gen.Return((elapsed, latencies))

    elapsed, latencies = run_on_ioloop(client, coro)

    print_latency_stats("Watchdog Latency", REQ_ITERATIONS, elapsed, latencies)
    p50, p95, p99 = percentiles(latencies)
    record("watchdog", iterations=REQ_ITERATIONS, elapsed=elapsed,
           rate=REQ_ITERATIONS / elapsed, p50=p50, p95=p95, p99=p99)

    client.stop(timeout=5); client.join(timeout=5)
    server.stop(timeout=5); server.join(timeout=5)


# ---------------------------------------------------------------------------
# 2. Request latency: ?sensor-value (single)
# ---------------------------------------------------------------------------

def bench_02_sensor_value_single_latency():
    """Benchmark sensor value request latency for a single sensor."""
    server = BenchServer("127.0.0.1", 0, num_sensors=10)
    server.start(timeout=5)
    port = server.bind_address[1]
    client = connect_client(port)

    for i in range(10):
        server.get_sensor("sensor-{}".format(i)).set_value(i, Sensor.NOMINAL)

    @tornado.gen.coroutine
    def coro():
        for _ in range(WARMUP):
            yield client.future_request(
                Message.request("sensor-value", "sensor-0"), timeout=5)

        latencies = []
        start = time.time()
        for _ in range(REQ_ITERATIONS):
            t = time.time()
            reply, informs = yield client.future_request(
                Message.request("sensor-value", "sensor-0"), timeout=5)
            latencies.append((time.time() - t) * 1e6)
            assert reply.reply_ok(), reply
            assert len(informs) == 1
        elapsed = time.time() - start
        raise tornado.gen.Return((elapsed, latencies))

    elapsed, latencies = run_on_ioloop(client, coro)

    print_latency_stats("Sensor-Value (single) Latency", REQ_ITERATIONS,
                        elapsed, latencies)
    p50, p95, p99 = percentiles(latencies)
    record("sensor_value_single", iterations=REQ_ITERATIONS, elapsed=elapsed,
           rate=REQ_ITERATIONS / elapsed, p50=p50, p95=p95, p99=p99)

    client.stop(timeout=5); client.join(timeout=5)
    server.stop(timeout=5); server.join(timeout=5)


# ---------------------------------------------------------------------------
# 3. Request latency: ?sensor-value (all 10)
# ---------------------------------------------------------------------------

def bench_03_sensor_value_all_latency():
    """Benchmark sensor value request latency for all sensors."""
    server = BenchServer("127.0.0.1", 0, num_sensors=10)
    server.start(timeout=5)
    port = server.bind_address[1]
    client = connect_client(port)

    for i in range(10):
        server.get_sensor("sensor-{}".format(i)).set_value(i, Sensor.NOMINAL)

    @tornado.gen.coroutine
    def coro():
        for _ in range(WARMUP):
            yield client.future_request(Message.request("sensor-value"), timeout=5)

        latencies = []
        start = time.time()
        for _ in range(HELP_ITERATIONS):
            t = time.time()
            reply, informs = yield client.future_request(
                Message.request("sensor-value"), timeout=5)
            latencies.append((time.time() - t) * 1e6)
            assert reply.reply_ok(), reply
            assert len(informs) == 10
        elapsed = time.time() - start
        raise tornado.gen.Return((elapsed, latencies))

    elapsed, latencies = run_on_ioloop(client, coro)

    print_latency_stats("Sensor-Value (all 10) Latency", HELP_ITERATIONS,
                        elapsed, latencies)
    p50, p95, p99 = percentiles(latencies)
    record("sensor_value_all_10", iterations=HELP_ITERATIONS, elapsed=elapsed,
           rate=HELP_ITERATIONS / elapsed, p50=p50, p95=p95, p99=p99)

    client.stop(timeout=5); client.join(timeout=5)
    server.stop(timeout=5); server.join(timeout=5)


# ---------------------------------------------------------------------------
# 4. Request latency: ?help
# ---------------------------------------------------------------------------

def bench_04_help_latency():
    """Benchmark help latency."""
    server = BenchServer("127.0.0.1", 0, num_sensors=0)
    server.start(timeout=5)
    port = server.bind_address[1]
    client = connect_client(port)

    @tornado.gen.coroutine
    def coro():
        for _ in range(WARMUP):
            yield client.future_request(Message.request("help"), timeout=5)

        latencies = []
        start = time.time()
        for _ in range(HELP_ITERATIONS):
            t = time.time()
            reply, informs = yield client.future_request(
                Message.request("help"), timeout=5)
            latencies.append((time.time() - t) * 1e6)
            assert reply.reply_ok(), reply
            assert len(informs) >= 5
        elapsed = time.time() - start
        raise tornado.gen.Return((elapsed, latencies))

    elapsed, latencies = run_on_ioloop(client, coro)

    print_latency_stats("Help Latency", HELP_ITERATIONS, elapsed, latencies)
    p50, p95, p99 = percentiles(latencies)
    record("help", iterations=HELP_ITERATIONS, elapsed=elapsed,
           rate=HELP_ITERATIONS / elapsed, p50=p50, p95=p95, p99=p99)

    client.stop(timeout=5); client.join(timeout=5)
    server.stop(timeout=5); server.join(timeout=5)


# ---------------------------------------------------------------------------
# 5. Concurrent clients
# ---------------------------------------------------------------------------

def bench_05_concurrent_clients():
    """Benchmark concurrent clients using async future_request."""
    server = BenchServer("127.0.0.1", 0, num_sensors=1)
    server.start(timeout=5)
    port = server.bind_address[1]

    clients = [connect_client(port) for _ in range(CONCURRENT_CLIENTS)]

    # Drive each client on its own ioloop with future_request
    results_list = [None] * CONCURRENT_CLIENTS
    barrier = threading.Barrier(CONCURRENT_CLIENTS + 1)

    def run_client(idx, c, n):
        done_evt = threading.Event()

        @tornado.gen.coroutine
        def work():
            barrier.wait()
            for _ in range(n):
                reply, _ = yield c.future_request(
                    Message.request("watchdog"), timeout=5)
                assert reply.reply_ok(), reply
            results_list[idx] = n
            done_evt.set()

        c.ioloop.add_callback(work)
        return done_evt

    evts = []
    for i, c in enumerate(clients):
        evts.append(run_client(i, c, CONCURRENT_REQS_PER_CLIENT))

    start = time.time()
    barrier.wait()
    for e in evts:
        e.wait(timeout=60)
    elapsed = time.time() - start

    total = CONCURRENT_CLIENTS * CONCURRENT_REQS_PER_CLIENT
    print()
    print("=== Concurrent Clients ({} clients x {} reqs) ===".format(
        CONCURRENT_CLIENTS, CONCURRENT_REQS_PER_CLIENT))
    print("  {} total requests in {:.3f}s = {} aggregate".format(
        total, elapsed, format_rate(total, elapsed)))
    print("  Per-client: {}".format(
        format_rate(CONCURRENT_REQS_PER_CLIENT, elapsed)))
    record("concurrent_clients", total=total, elapsed=elapsed,
           rate=total / elapsed,
           per_client_rate=CONCURRENT_REQS_PER_CLIENT / elapsed)

    for c in clients:
        c.stop(timeout=5); c.join(timeout=5)
    server.stop(timeout=5); server.join(timeout=5)


# ---------------------------------------------------------------------------
# 6. Sensor delivery: push fixed N, measure delivery
# ---------------------------------------------------------------------------

def bench_06_sensor_delivery():
    """Benchmark sensor delivery speeds."""
    server = BenchServer("127.0.0.1", 0, num_sensors=1)
    server.start(timeout=5)
    port = server.bind_address[1]
    client = connect_client(port)

    received = [0]
    lock = threading.Lock()

    def on_sensor_status(client_self, msg):
        with lock:
            received[0] += 1

    client._inform_handlers["sensor-status"] = on_sensor_status

    @tornado.gen.coroutine
    def subscribe():
        reply, _ = yield client.future_request(
            Message.request("sensor-sampling", "sensor-0", "event"), timeout=5)
        assert reply.reply_ok(), reply

    run_on_ioloop(client, subscribe)

    # Let subscription settle, then reset
    time.sleep(0.1)
    with lock:
        received[0] = 0

    sensor = server.get_sensor("sensor-0")

    start = time.time()
    for i in range(DELIVERY_UPDATES):
        sensor.set_value(i, Sensor.NOMINAL)
    push_elapsed = time.time() - start

    # Wait for drain
    time.sleep(2.0)
    with lock:
        total_received = received[0]

    print()
    print("=== Sensor Delivery ({} updates, 1 sensor) ===".format(DELIVERY_UPDATES))
    print("  Push: {:.3f}s = {}".format(push_elapsed,
          format_rate(DELIVERY_UPDATES, push_elapsed)))
    print("  Delivered: {} / {} ({:.1f}%)".format(
        total_received, DELIVERY_UPDATES,
        total_received / DELIVERY_UPDATES * 100))
    record("sensor_delivery", updates=DELIVERY_UPDATES,
           push_time=push_elapsed, push_rate=DELIVERY_UPDATES / push_elapsed,
           received=total_received,
           delivery_pct=total_received / DELIVERY_UPDATES * 100)

    client.stop(timeout=5); client.join(timeout=5)
    server.stop(timeout=5); server.join(timeout=5)


# ---------------------------------------------------------------------------
# 7. Multi-sensor delivery
# ---------------------------------------------------------------------------

def bench_07_multi_sensor_delivery():
    """Benchmark multi sensor delivery speeds."""
    server = BenchServer("127.0.0.1", 0, num_sensors=MULTI_SENSOR_COUNT)
    server.start(timeout=5)
    port = server.bind_address[1]
    client = connect_client(port)

    received = [0]
    lock = threading.Lock()

    def on_sensor_status(client_self, msg):
        with lock:
            received[0] += 1

    client._inform_handlers["sensor-status"] = on_sensor_status

    @tornado.gen.coroutine
    def subscribe_all():
        for i in range(MULTI_SENSOR_COUNT):
            reply, _ = yield client.future_request(
                Message.request("sensor-sampling",
                                "sensor-{}".format(i), "event"),
                timeout=5)
            assert reply.reply_ok(), reply

    run_on_ioloop(client, subscribe_all)

    # Let subscription settle, then reset
    time.sleep(0.1)
    with lock:
        received[0] = 0

    total_updates = MULTI_SENSOR_UPDATES_PER * MULTI_SENSOR_COUNT

    start = time.time()
    for r in range(MULTI_SENSOR_UPDATES_PER):
        for i in range(MULTI_SENSOR_COUNT):
            server.get_sensor("sensor-{}".format(i)).set_value(
                r * MULTI_SENSOR_COUNT + i, Sensor.NOMINAL)
    push_elapsed = time.time() - start

    time.sleep(2.0)
    with lock:
        total_received = received[0]

    print()
    print("=== Multi-Sensor Delivery ({} sensors x {} = {} updates) ===".format(
        MULTI_SENSOR_COUNT, MULTI_SENSOR_UPDATES_PER, total_updates))
    print("  Push: {:.3f}s = {}".format(push_elapsed,
          format_rate(total_updates, push_elapsed)))
    print("  Delivered: {} / {} ({:.1f}%)".format(
        total_received, total_updates,
        total_received / total_updates * 100))
    record("multi_sensor_delivery", num_sensors=MULTI_SENSOR_COUNT,
           total_updates=total_updates, push_time=push_elapsed,
           push_rate=total_updates / push_elapsed,
           received=total_received,
           delivery_pct=total_received / total_updates * 100)

    client.stop(timeout=5); client.join(timeout=5)
    server.stop(timeout=5); server.join(timeout=5)


# ---------------------------------------------------------------------------
# 8. High sensor count operations
# ---------------------------------------------------------------------------

def bench_08_high_sensor_count():
    """Benchmark behavior with a high sensor count."""
    server = BenchServer("127.0.0.1", 0, num_sensors=HIGH_SENSOR_COUNT)
    server.start(timeout=10)
    port = server.bind_address[1]
    client = connect_client(port, timeout=30)

    for i in range(HIGH_SENSOR_COUNT):
        server.get_sensor("sensor-{}".format(i)).set_value(i, Sensor.NOMINAL)

    @tornado.gen.coroutine
    def coro():
        # sensor-list all
        start = time.time()
        for _ in range(HIGH_SENSOR_LIST_ITERS):
            reply, informs = yield client.future_request(
                Message.request("sensor-list"), timeout=30)
            assert reply.reply_ok(), reply
            assert len(informs) == HIGH_SENSOR_COUNT
        list_elapsed = time.time() - start

        # sensor-value all
        start = time.time()
        for _ in range(HIGH_SENSOR_VALUE_ALL_ITERS):
            reply, informs = yield client.future_request(
                Message.request("sensor-value"), timeout=30)
            assert reply.reply_ok(), reply
            assert len(informs) == HIGH_SENSOR_COUNT
        val_all_elapsed = time.time() - start

        # sensor-value single
        start = time.time()
        for _ in range(HIGH_SENSOR_VALUE_SINGLE_ITERS):
            reply, informs = yield client.future_request(
                Message.request("sensor-value", "sensor-500"), timeout=10)
            assert reply.reply_ok(), reply
        val_single_elapsed = time.time() - start

        # sensor-list regex
        start = time.time()
        for _ in range(HIGH_SENSOR_REGEX_ITERS):
            reply, informs = yield client.future_request(
                Message.request("sensor-list", "/sensor-1.*/"), timeout=10)
            assert reply.reply_ok(), reply
            assert len(informs) > 0
        regex_elapsed = time.time() - start

        raise tornado.gen.Return((list_elapsed, val_all_elapsed,
                                  val_single_elapsed, regex_elapsed))

    list_elapsed, val_all_elapsed, val_single_elapsed, regex_elapsed = \
        run_on_ioloop(client, coro)

    print()
    print("=== High Sensor Count ({} sensors) ===".format(HIGH_SENSOR_COUNT))

    print("  sensor-list (all): {} reqs in {:.3f}s = {}".format(
        HIGH_SENSOR_LIST_ITERS, list_elapsed,
        format_rate(HIGH_SENSOR_LIST_ITERS, list_elapsed)))
    r_list = {"iters": HIGH_SENSOR_LIST_ITERS, "elapsed": list_elapsed,
              "rate": HIGH_SENSOR_LIST_ITERS / list_elapsed}

    print("  sensor-value (all): {} reqs in {:.3f}s = {}".format(
        HIGH_SENSOR_VALUE_ALL_ITERS, val_all_elapsed,
        format_rate(HIGH_SENSOR_VALUE_ALL_ITERS, val_all_elapsed)))
    r_val_all = {"iters": HIGH_SENSOR_VALUE_ALL_ITERS, "elapsed": val_all_elapsed,
                 "rate": HIGH_SENSOR_VALUE_ALL_ITERS / val_all_elapsed}

    print("  sensor-value (single): {} reqs in {:.3f}s = {}".format(
        HIGH_SENSOR_VALUE_SINGLE_ITERS, val_single_elapsed,
        format_rate(HIGH_SENSOR_VALUE_SINGLE_ITERS, val_single_elapsed)))
    r_val_single = {"iters": HIGH_SENSOR_VALUE_SINGLE_ITERS,
                    "elapsed": val_single_elapsed,
                    "rate": HIGH_SENSOR_VALUE_SINGLE_ITERS / val_single_elapsed}

    print("  sensor-list (regex): {} reqs in {:.3f}s = {}".format(
        HIGH_SENSOR_REGEX_ITERS, regex_elapsed,
        format_rate(HIGH_SENSOR_REGEX_ITERS, regex_elapsed)))
    r_regex = {"iters": HIGH_SENSOR_REGEX_ITERS, "elapsed": regex_elapsed,
               "rate": HIGH_SENSOR_REGEX_ITERS / regex_elapsed}

    record("high_sensor_count",
           sensor_list=r_list, sensor_value_all=r_val_all,
           sensor_value_single=r_val_single, sensor_list_regex=r_regex)

    client.stop(timeout=5); client.join(timeout=5)
    server.stop(timeout=5); server.join(timeout=5)


# ---------------------------------------------------------------------------
# 9. Scale: 20K sensors
# ---------------------------------------------------------------------------

def bench_09_scale_20k_sensors():
    """Benchmark scaling behavior with 20k sensors."""
    reg_start = time.time()
    server = BenchServer("127.0.0.1", 0, num_sensors=SCALE_SENSOR_COUNT)
    reg_elapsed = time.time() - reg_start

    server.start(timeout=30)
    port = server.bind_address[1]

    client = connect_client(port, timeout=60)

    received = [0]
    lock = threading.Lock()

    def on_sensor_status(client_self, msg):
        with lock:
            received[0] += 1

    client._inform_handlers["sensor-status"] = on_sensor_status

    # Subscribe all using future_request
    @tornado.gen.coroutine
    def subscribe_all():
        for i in range(SCALE_SENSOR_COUNT):
            reply, _ = yield client.future_request(
                Message.request("sensor-sampling",
                                "sensor-{}".format(i), "event"),
                timeout=30)
            assert reply.reply_ok(), "sensor-{}: {}".format(i, reply)

    sub_start = time.time()
    run_on_ioloop(client, subscribe_all)
    sub_elapsed = time.time() - sub_start

    # Reset counter after subscribe phase
    time.sleep(0.2)
    with lock:
        received[0] = 0

    sensors = [server.get_sensor("sensor-{}".format(i))
               for i in range(SCALE_SENSOR_COUNT)]

    # Push
    total_pushed = SCALE_ROUNDS * SCALE_SENSOR_COUNT
    push_start = time.time()
    for r in range(SCALE_ROUNDS):
        for i, s in enumerate(sensors):
            s.set_value(r * SCALE_SENSOR_COUNT + i, Sensor.NOMINAL)
    push_elapsed = time.time() - push_start

    # Wait for drain
    time.sleep(5.0)
    with lock:
        total_received = received[0]

    # Bulk queries using future_request
    @tornado.gen.coroutine
    def bulk_queries():
        list_start = time.time()
        reply, informs = yield client.future_request(
            Message.request("sensor-list"), timeout=60)
        list_elapsed = time.time() - list_start
        assert reply.reply_ok(), reply
        assert len(informs) == SCALE_SENSOR_COUNT

        value_start = time.time()
        reply, informs = yield client.future_request(
            Message.request("sensor-value"), timeout=60)
        value_elapsed = time.time() - value_start
        assert reply.reply_ok(), reply
        assert len(informs) == SCALE_SENSOR_COUNT

        raise tornado.gen.Return((list_elapsed, value_elapsed))

    list_elapsed, value_elapsed = run_on_ioloop(client, bulk_queries)

    print()
    print("=== Scale: {} Sensors ===".format(SCALE_SENSOR_COUNT))
    print("  Registration: {:.3f}s".format(reg_elapsed))
    print("  Subscribe all: {:.3f}s = {}".format(
        sub_elapsed, format_rate(SCALE_SENSOR_COUNT, sub_elapsed)))
    print("  Push {} updates: {:.3f}s = {}".format(
        total_pushed, push_elapsed,
        format_rate(total_pushed, push_elapsed)))
    print("  Delivered: {} / {} ({:.1f}%)".format(
        total_received, total_pushed,
        total_received / total_pushed * 100))
    print("  sensor-list (all): {:.3f}s".format(list_elapsed))
    print("  sensor-value (all): {:.3f}s".format(value_elapsed))

    record("scale_20k",
           reg_time=reg_elapsed,
           sub_time=sub_elapsed,
           sub_rate=SCALE_SENSOR_COUNT / sub_elapsed,
           push_total=total_pushed, push_time=push_elapsed,
           push_rate=total_pushed / push_elapsed,
           received=total_received,
           delivery_pct=total_received / total_pushed * 100,
           sensor_list_time=list_elapsed,
           sensor_value_time=value_elapsed)

    client.stop(timeout=5); client.join(timeout=5)
    server.stop(timeout=10); server.join(timeout=10)


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

if __name__ == "__main__":
    print("=" * 60)
    print("KATCP-Python Performance Benchmarks (async)")
    print("Python {}".format(sys.version.split()[0]))
    print("katcp {}".format(katcp.__version__))
    print("=" * 60)

    benchmarks = [
        ("01 Watchdog Latency", bench_01_watchdog_latency),
        ("02 Sensor-Value Single", bench_02_sensor_value_single_latency),
        ("03 Sensor-Value All 10", bench_03_sensor_value_all_latency),
        ("04 Help Latency", bench_04_help_latency),
        ("05 Concurrent Clients", bench_05_concurrent_clients),
        ("06 Sensor Delivery", bench_06_sensor_delivery),
        ("07 Multi-Sensor Delivery", bench_07_multi_sensor_delivery),
        ("08 High Sensor Count", bench_08_high_sensor_count),
        ("09 Scale 20K Sensors", bench_09_scale_20k_sensors),
    ]

    for name, bench_fn in benchmarks:
        try:
            bench_fn()
        except Exception as e:
            print("\n!!! {} FAILED: {}".format(name, e))
            import traceback
            traceback.print_exc()

    with open("tests/bench_katcp_python_results.json", "w") as f:
        json.dump(results, f, indent=2)

    print()
    print("=" * 60)
    print("Results saved to tests/bench_katcp_python_results.json")
    print("=" * 60)
