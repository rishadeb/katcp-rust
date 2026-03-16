#!/usr/bin/env python3
"""
Performance benchmarks for aiokatcp, directly comparable with katcp-rust and katcp-python.

Both suites use:
  - Identical iteration counts and sensor counts
  - Idiomatic API usage for each language
  - Same warmup procedures
  - Latency percentiles (p50/p95/p99) for request tests
  - Fixed-N push + drain for delivery tests (no artificial throttling)

Run with:
    source aiokatcp_venv/bin/activate
    python tests/bench_aiokatcp.py
"""

import asyncio
import json
import logging
import sys
import time

# Suppress noisy logging during benchmarks
logging.basicConfig(level=logging.CRITICAL)

import aiokatcp
from aiokatcp import Client, DeviceServer, Sensor
from aiokatcp.core import Timestamp

# ---------------------------------------------------------------------------
# Constants — must match the Rust and katcp-python suites exactly
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
    VERSION = "bench-server-1.0"
    BUILD_STATE = "bench-build-1.0.0"


def create_server(num_sensors=0):
    """Create a bench server with the given number of sensors."""
    server = BenchServer("127.0.0.1", 0)
    for i in range(num_sensors):
        sensor = Sensor(int, f"sensor-{i}", "bench sensor", default=0)
        server.sensors.add(sensor)
    return server


def format_rate(count, elapsed):
    """Format a rate into a string."""
    rate = count / elapsed if elapsed > 0 else 0
    if rate >= 1e6:
        return f"{rate / 1e6:.2f}M/s"
    elif rate >= 1e3:
        return f"{rate / 1e3:.2f}K/s"
    else:
        return f"{rate:.2f}/s"


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
    print(f"=== {name} ===")
    print(f"  {iterations} requests in {elapsed:.3f}s = {format_rate(iterations, elapsed)}")
    print(f"  Latency: p50={p50:.0f}us  p95={p95:.0f}us  p99={p99:.0f}us")


results = {}


def record(name, **kwargs):
    """Record metrics into results dictionary."""
    results[name] = kwargs


# ---------------------------------------------------------------------------
# 1. Request latency: ?watchdog
# ---------------------------------------------------------------------------

async def bench_01_watchdog_latency():
    """Benchmark watchdog request latency."""
    server = create_server(num_sensors=0)
    await server.start()
    port = server.sockets[0].getsockname()[1]
    client = await Client.connect("127.0.0.1", port)

    for _ in range(WARMUP):
        await client.request("watchdog")

    latencies = []
    start = time.time()
    for _ in range(REQ_ITERATIONS):
        t = time.time()
        await client.request("watchdog")
        latencies.append((time.time() - t) * 1e6)  # us
    elapsed = time.time() - start

    print_latency_stats("Watchdog Latency", REQ_ITERATIONS, elapsed, latencies)
    p50, p95, p99 = percentiles(latencies)
    record("watchdog", iterations=REQ_ITERATIONS, elapsed=elapsed,
           rate=REQ_ITERATIONS / elapsed, p50=p50, p95=p95, p99=p99)

    client.close()
    await client.wait_closed()
    server.halt()
    await server.join()


# ---------------------------------------------------------------------------
# 2. Request latency: ?sensor-value (single)
# ---------------------------------------------------------------------------

async def bench_02_sensor_value_single_latency():
    """Benchmark sensor value request latency for a single sensor."""
    server = create_server(num_sensors=10)
    await server.start()
    port = server.sockets[0].getsockname()[1]
    client = await Client.connect("127.0.0.1", port)

    for i in range(10):
        server.sensors[f"sensor-{i}"].value = i

    for _ in range(WARMUP):
        await client.request("sensor-value", "sensor-0")

    latencies = []
    start = time.time()
    for _ in range(REQ_ITERATIONS):
        t = time.time()
        reply, informs = await client.request("sensor-value", "sensor-0")
        latencies.append((time.time() - t) * 1e6)
        assert len(informs) == 1
    elapsed = time.time() - start

    print_latency_stats("Sensor-Value (single) Latency", REQ_ITERATIONS,
                        elapsed, latencies)
    p50, p95, p99 = percentiles(latencies)
    record("sensor_value_single", iterations=REQ_ITERATIONS, elapsed=elapsed,
           rate=REQ_ITERATIONS / elapsed, p50=p50, p95=p95, p99=p99)

    client.close()
    await client.wait_closed()
    server.halt()
    await server.join()


# ---------------------------------------------------------------------------
# 3. Request latency: ?sensor-value (all 10)
# ---------------------------------------------------------------------------

async def bench_03_sensor_value_all_latency():
    """Benchmark sensor value request latency for all sensors."""
    server = create_server(num_sensors=10)
    await server.start()
    port = server.sockets[0].getsockname()[1]
    client = await Client.connect("127.0.0.1", port)

    for i in range(10):
        server.sensors[f"sensor-{i}"].value = i

    for _ in range(WARMUP):
        await client.request("sensor-value")

    latencies = []
    start = time.time()
    for _ in range(HELP_ITERATIONS):
        t = time.time()
        reply, informs = await client.request("sensor-value")
        latencies.append((time.time() - t) * 1e6)
        assert len(informs) == 10
    elapsed = time.time() - start

    print_latency_stats("Sensor-Value (all 10) Latency", HELP_ITERATIONS,
                        elapsed, latencies)
    p50, p95, p99 = percentiles(latencies)
    record("sensor_value_all_10", iterations=HELP_ITERATIONS, elapsed=elapsed,
           rate=HELP_ITERATIONS / elapsed, p50=p50, p95=p95, p99=p99)

    client.close()
    await client.wait_closed()
    server.halt()
    await server.join()


# ---------------------------------------------------------------------------
# 4. Request latency: ?help
# ---------------------------------------------------------------------------

async def bench_04_help_latency():
    """Benchmark help latency."""
    server = create_server(num_sensors=0)
    await server.start()
    port = server.sockets[0].getsockname()[1]
    client = await Client.connect("127.0.0.1", port)

    for _ in range(WARMUP):
        await client.request("help")

    latencies = []
    start = time.time()
    for _ in range(HELP_ITERATIONS):
        t = time.time()
        reply, informs = await client.request("help")
        latencies.append((time.time() - t) * 1e6)
        assert len(informs) >= 5
    elapsed = time.time() - start

    print_latency_stats("Help Latency", HELP_ITERATIONS, elapsed, latencies)
    p50, p95, p99 = percentiles(latencies)
    record("help", iterations=HELP_ITERATIONS, elapsed=elapsed,
           rate=HELP_ITERATIONS / elapsed, p50=p50, p95=p95, p99=p99)

    client.close()
    await client.wait_closed()
    server.halt()
    await server.join()


# ---------------------------------------------------------------------------
# 5. Concurrent clients
# ---------------------------------------------------------------------------

async def bench_05_concurrent_clients():
    """Benchmark concurrent clients."""
    server = create_server(num_sensors=1)
    await server.start()
    port = server.sockets[0].getsockname()[1]

    clients = []
    for _ in range(CONCURRENT_CLIENTS):
        c = await Client.connect("127.0.0.1", port)
        clients.append(c)

    async def client_worker(c, n):
        for _ in range(n):
            await c.request("watchdog")

    # Warmup
    await asyncio.gather(*[client_worker(c, 10) for c in clients])

    start = time.time()
    await asyncio.gather(
        *[client_worker(c, CONCURRENT_REQS_PER_CLIENT) for c in clients]
    )
    elapsed = time.time() - start

    total = CONCURRENT_CLIENTS * CONCURRENT_REQS_PER_CLIENT
    print()
    print(f"=== Concurrent Clients ({CONCURRENT_CLIENTS} clients x {CONCURRENT_REQS_PER_CLIENT} reqs) ===")
    print(f"  {total} total requests in {elapsed:.3f}s = {format_rate(total, elapsed)} aggregate")
    print(f"  Per-client: {format_rate(CONCURRENT_REQS_PER_CLIENT, elapsed)}")
    record("concurrent_clients", total=total, elapsed=elapsed,
           rate=total / elapsed,
           per_client_rate=CONCURRENT_REQS_PER_CLIENT / elapsed)

    for c in clients:
        c.close()
        await c.wait_closed()
    server.halt()
    await server.join()


# ---------------------------------------------------------------------------
# 6. Sensor delivery: push fixed N, measure delivery
# ---------------------------------------------------------------------------

async def bench_06_sensor_delivery():
    """Benchmark sensor delivery speeds."""
    server = create_server(num_sensors=1)
    await server.start()
    port = server.sockets[0].getsockname()[1]
    client = await Client.connect("127.0.0.1", port)

    received = [0]

    def on_inform(timestamp: Timestamp, count: int, name: str,
                  status: str, value: bytes):
        received[0] += 1

    client.add_inform_callback("sensor-status", on_inform)

    await client.request("sensor-sampling", "sensor-0", "event")

    # Let subscription settle, then reset
    await asyncio.sleep(0.1)
    received[0] = 0

    sensor = server.sensors["sensor-0"]

    start = time.time()
    for i in range(DELIVERY_UPDATES):
        sensor.value = i
    push_elapsed = time.time() - start

    # Wait for drain
    await asyncio.sleep(2.0)
    total_received = received[0]

    print()
    print(f"=== Sensor Delivery ({DELIVERY_UPDATES} updates, 1 sensor) ===")
    print(f"  Push: {push_elapsed:.3f}s = {format_rate(DELIVERY_UPDATES, push_elapsed)}")
    print(f"  Delivered: {total_received} / {DELIVERY_UPDATES} ({total_received / DELIVERY_UPDATES * 100:.1f}%)")
    record("sensor_delivery", updates=DELIVERY_UPDATES,
           push_time=push_elapsed, push_rate=DELIVERY_UPDATES / push_elapsed,
           received=total_received,
           delivery_pct=total_received / DELIVERY_UPDATES * 100)

    client.close()
    await client.wait_closed()
    server.halt()
    await server.join()


# ---------------------------------------------------------------------------
# 7. Multi-sensor delivery
# ---------------------------------------------------------------------------

async def bench_07_multi_sensor_delivery():
    """Benchmark multi sensor delivery speeds."""
    server = create_server(num_sensors=MULTI_SENSOR_COUNT)
    await server.start()
    port = server.sockets[0].getsockname()[1]
    client = await Client.connect("127.0.0.1", port)

    received = [0]

    def on_inform(timestamp: Timestamp, count: int, name: str,
                  status: str, value: bytes):
        received[0] += 1

    client.add_inform_callback("sensor-status", on_inform)

    for i in range(MULTI_SENSOR_COUNT):
        await client.request("sensor-sampling", f"sensor-{i}", "event")

    # Let subscription settle, then reset
    await asyncio.sleep(0.1)
    received[0] = 0

    total_updates = MULTI_SENSOR_UPDATES_PER * MULTI_SENSOR_COUNT

    start = time.time()
    for r in range(MULTI_SENSOR_UPDATES_PER):
        for i in range(MULTI_SENSOR_COUNT):
            server.sensors[f"sensor-{i}"].value = r * MULTI_SENSOR_COUNT + i
    push_elapsed = time.time() - start

    await asyncio.sleep(2.0)
    total_received = received[0]

    print()
    print(f"=== Multi-Sensor Delivery ({MULTI_SENSOR_COUNT} sensors x {MULTI_SENSOR_UPDATES_PER} = {total_updates} updates) ===")
    print(f"  Push: {push_elapsed:.3f}s = {format_rate(total_updates, push_elapsed)}")
    print(f"  Delivered: {total_received} / {total_updates} ({total_received / total_updates * 100:.1f}%)")
    record("multi_sensor_delivery", num_sensors=MULTI_SENSOR_COUNT,
           total_updates=total_updates, push_time=push_elapsed,
           push_rate=total_updates / push_elapsed,
           received=total_received,
           delivery_pct=total_received / total_updates * 100)

    client.close()
    await client.wait_closed()
    server.halt()
    await server.join()


# ---------------------------------------------------------------------------
# 8. High sensor count operations
# ---------------------------------------------------------------------------

async def bench_08_high_sensor_count():
    """Benchmark behavior with a high sensor count."""
    server = create_server(num_sensors=HIGH_SENSOR_COUNT)
    await server.start()
    port = server.sockets[0].getsockname()[1]
    client = await Client.connect("127.0.0.1", port)

    for i in range(HIGH_SENSOR_COUNT):
        server.sensors[f"sensor-{i}"].value = i

    # sensor-list all
    start = time.time()
    for _ in range(HIGH_SENSOR_LIST_ITERS):
        reply, informs = await client.request("sensor-list")
        assert len(informs) == HIGH_SENSOR_COUNT
    elapsed = time.time() - start

    print()
    print(f"=== High Sensor Count ({HIGH_SENSOR_COUNT} sensors) ===")
    print(f"  sensor-list (all): {HIGH_SENSOR_LIST_ITERS} reqs in {elapsed:.3f}s = {format_rate(HIGH_SENSOR_LIST_ITERS, elapsed)}")
    r_list = {"iters": HIGH_SENSOR_LIST_ITERS, "elapsed": elapsed,
              "rate": HIGH_SENSOR_LIST_ITERS / elapsed}

    # sensor-value all
    start = time.time()
    for _ in range(HIGH_SENSOR_VALUE_ALL_ITERS):
        reply, informs = await client.request("sensor-value")
        assert len(informs) == HIGH_SENSOR_COUNT
    elapsed = time.time() - start

    print(f"  sensor-value (all): {HIGH_SENSOR_VALUE_ALL_ITERS} reqs in {elapsed:.3f}s = {format_rate(HIGH_SENSOR_VALUE_ALL_ITERS, elapsed)}")
    r_val_all = {"iters": HIGH_SENSOR_VALUE_ALL_ITERS, "elapsed": elapsed,
                 "rate": HIGH_SENSOR_VALUE_ALL_ITERS / elapsed}

    # sensor-value single
    start = time.time()
    for _ in range(HIGH_SENSOR_VALUE_SINGLE_ITERS):
        reply, informs = await client.request("sensor-value", "sensor-500")
    elapsed = time.time() - start

    print(f"  sensor-value (single): {HIGH_SENSOR_VALUE_SINGLE_ITERS} reqs in {elapsed:.3f}s = {format_rate(HIGH_SENSOR_VALUE_SINGLE_ITERS, elapsed)}")
    r_val_single = {"iters": HIGH_SENSOR_VALUE_SINGLE_ITERS,
                    "elapsed": elapsed,
                    "rate": HIGH_SENSOR_VALUE_SINGLE_ITERS / elapsed}

    # sensor-list regex
    start = time.time()
    for _ in range(HIGH_SENSOR_REGEX_ITERS):
        reply, informs = await client.request("sensor-list", "/sensor-1.*/")
        assert len(informs) > 0
    elapsed = time.time() - start

    print(f"  sensor-list (regex): {HIGH_SENSOR_REGEX_ITERS} reqs in {elapsed:.3f}s = {format_rate(HIGH_SENSOR_REGEX_ITERS, elapsed)}")
    r_regex = {"iters": HIGH_SENSOR_REGEX_ITERS, "elapsed": elapsed,
               "rate": HIGH_SENSOR_REGEX_ITERS / elapsed}

    record("high_sensor_count",
           sensor_list=r_list, sensor_value_all=r_val_all,
           sensor_value_single=r_val_single, sensor_list_regex=r_regex)

    client.close()
    await client.wait_closed()
    server.halt()
    await server.join()


# ---------------------------------------------------------------------------
# 9. Scale: 20K sensors
# ---------------------------------------------------------------------------

async def bench_09_scale_20k_sensors():
    """Benchmark scaling behavior with 20k sensors."""
    reg_start = time.time()
    server = create_server(num_sensors=SCALE_SENSOR_COUNT)
    reg_elapsed = time.time() - reg_start

    await server.start()
    port = server.sockets[0].getsockname()[1]

    client = await Client.connect("127.0.0.1", port)

    received = [0]

    def on_inform(timestamp: Timestamp, count: int, name: str,
                  status: str, value: bytes):
        received[0] += 1

    client.add_inform_callback("sensor-status", on_inform)

    # Subscribe all
    sub_start = time.time()
    for i in range(SCALE_SENSOR_COUNT):
        await client.request("sensor-sampling", f"sensor-{i}", "event")
    sub_elapsed = time.time() - sub_start

    # Reset counter after subscribe phase
    await asyncio.sleep(0.2)
    received[0] = 0

    # Push
    total_pushed = SCALE_ROUNDS * SCALE_SENSOR_COUNT
    push_start = time.time()
    for r in range(SCALE_ROUNDS):
        for i in range(SCALE_SENSOR_COUNT):
            server.sensors[f"sensor-{i}"].value = r * SCALE_SENSOR_COUNT + i
    push_elapsed = time.time() - push_start

    # Wait for drain
    await asyncio.sleep(5.0)
    total_received = received[0]

    # Bulk queries
    list_start = time.time()
    reply, informs = await client.request("sensor-list")
    list_elapsed = time.time() - list_start
    assert len(informs) == SCALE_SENSOR_COUNT

    value_start = time.time()
    reply, informs = await client.request("sensor-value")
    value_elapsed = time.time() - value_start
    assert len(informs) == SCALE_SENSOR_COUNT

    print()
    print(f"=== Scale: {SCALE_SENSOR_COUNT} Sensors ===")
    print(f"  Registration: {reg_elapsed:.3f}s")
    print(f"  Subscribe all: {sub_elapsed:.3f}s = {format_rate(SCALE_SENSOR_COUNT, sub_elapsed)}")
    print(f"  Push {total_pushed} updates: {push_elapsed:.3f}s = {format_rate(total_pushed, push_elapsed)}")
    print(f"  Delivered: {total_received} / {total_pushed} ({total_received / total_pushed * 100:.1f}%)")
    print(f"  sensor-list (all): {list_elapsed:.3f}s")
    print(f"  sensor-value (all): {value_elapsed:.3f}s")

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

    client.close()
    await client.wait_closed()
    server.halt()
    await server.join()


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

async def main():
    print("=" * 60)
    print("aiokatcp Performance Benchmarks")
    print(f"Python {sys.version.split()[0]}")
    print(f"aiokatcp {aiokatcp.__version__}")
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
            await bench_fn()
        except Exception as e:
            print(f"\n!!! {name} FAILED: {e}")
            import traceback
            traceback.print_exc()

    with open("tests/bench_aiokatcp_results.json", "w") as f:
        json.dump(results, f, indent=2)

    print()
    print("=" * 60)
    print("Results saved to tests/bench_aiokatcp_results.json")
    print("=" * 60)


if __name__ == "__main__":
    asyncio.run(main())
