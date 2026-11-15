# SQLite Memory Usage Profiling Report

## Summary

Recent Maki changes introduced SQLite-backed storage (WAL mode) for input history, model preferences, theme state, and plugin storage. This report quantifies the RAM impact.

## Methodology

Profiling used SQLite's native `sqlite3_status(SQLITE_STATUS_MEMORY_USED)` API to measure actual heap allocations by the SQLite engine. Tests were run against the `maki-storage` crate (rusqlite 0.41.1, bundled SQLite).

## Key Findings

### 1. Per-Database Connection Cost

| Database Type | Memory per Connection |
|---|---|
| In-memory (no WAL) | ~60 KB |
| File-backed (WAL mode) | ~151 KB |
| WAL mode overhead | ~90 KB |

The WAL mode adds ~90 KB per connection for file management structures, SHM (shared memory) mapping, and additional page cache.

### 2. Pragma Impact (per connection)

| Pragma | Memory Impact |
|---|---|
| `foreign_keys = ON` | +0 bytes (compile-time, no runtime cost) |
| `cache_size = -8192` (8MB) | +5 KB (lazy allocation, grows under load up to 8MB) |
| `temp_store = memory` | +0 bytes (only allocates when temp tables are used) |
| `synchronous = NORMAL` | +0 bytes (durability setting, no memory impact) |

**Critical**: The `cache_size = -8192` pragma allows up to 8MB of page cache per connection. While lazily allocated, under heavy database usage this can grow significantly. With 8 connections, worst-case cache usage = 64MB.

### 3. Total SQLite Memory by Usage Scenario

| Scenario | Connections | Memory |
|---|---|---|
| Minimal (no plugins) | 3 core DBs | ~454 KB |
| Typical (5 plugins) | 3 core + 5 plugin | ~1.2 MB |
| Heavy (10 plugins) | 3 core + 10 plugin | ~2.0 MB |

### 4. Identified Issues

#### A. Pool Registry Never Releases Connections (Memory Leak Risk)
- `POOL_REGISTRY` is a static `OnceLock<Mutex<HashMap>>`
- Entries are never removed (except `clear_pools()` in tests)
- Each unique database path creates a permanent connection
- **Risk**: Unbounded memory growth if many different DB paths are used

#### B. Input History Opens New Connection on Every Save
- `InputHistory::save()` calls `Database::open(file)` each time
- Each open/close cycle re-initializes WAL mode
- Causes memory churn and potential fragmentation
- **Fix**: Use `get_pool()` instead of `Database::open()`

#### C. `DatabasePool::with_read()` Takes Exclusive Lock
- Despite the comment "can be parallel with other reads in WAL", it acquires a mutex
- This serializes all access, negating WAL's concurrent read benefit
- **Fix**: Use `RwLock` or separate read connections

#### D. Cache Size Too Large
- 8MB per connection is excessive for Maki's small databases
- Most databases hold < 100 rows
- **Fix**: Reduce to 2MB (`cache_size = -2048`) or make configurable

#### E. `temp_store = memory`
- Temp tables stored in RAM instead of disk
- Minimal impact for current usage patterns
- **Risk**: Large temp operations could spike memory

### 5. Recommendations

1. **Reduce cache_size**: Change from `-8192` (8MB) to `-2048` (2MB) per connection
   - Expected savings: up to 48MB in worst case (8 connections × 6MB reduction)

2. **Fix input_history to use connection pool**: Replace `Database::open()` with `get_pool()`
   - Eliminates connection churn on every save

3. **Add pool eviction**: Implement LRU or time-based eviction for the pool registry
   - Prevents unbounded growth

4. **Use RwLock for read operations**: Allow concurrent reads in WAL mode
   - Improves performance without increasing memory

5. **Consider consolidating databases**: Use a single SQLite database with multiple tables
   - Reduces per-connection overhead (151 KB × N connections)

6. **Add connection cleanup on shutdown**: Ensure all connections are properly closed
   - Prevents WAL sidecar file accumulation

### 6. Binary Size Impact

The `rusqlite` crate with `bundled` feature compiles SQLite into the binary. This adds approximately 1-2 MB to the binary size (SQLite library is ~1MB compiled).

### 7. Before/After Comparison

| Metric | Before SQLite (JSON files) | After SQLite (WAL mode) |
|---|---|---|
| Input history storage | In-memory VecDeque + JSON file | SQLite DB + connection |
| Model preferences | JSON file | SQLite DB + connection |
| Theme state | JSON file | SQLite DB + connection |
| Plugin storage | N/A (no DB API) | SQLite DB per plugin + connection |
| Baseline memory | ~0 (JSON parsed on demand) | ~454 KB (3 core DBs) |
| Per-plugin memory | ~0 | ~151 KB per plugin DB |

### 8. Conclusion

The SQLite migration adds approximately **450 KB - 2 MB** of baseline RAM usage depending on plugin count. The primary memory risks are:
1. Unbounded cache growth (up to 8MB per connection under load)
2. Pool registry never releasing connections
3. Input history connection churn on every save

These issues should be addressed to prevent memory growth in long-running sessions.
