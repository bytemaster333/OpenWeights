-- rate_limit.lua — atomic token-bucket check-and-take (OPS-04, CONTEXT D-21).
--
-- KEYS[1] = bucket count key        e.g. "rl:upload:<kid>"
-- KEYS[2] = last-refill-timestamp   e.g. "rl:upload:<kid>:ts"
-- ARGV[1] = capacity (integer tokens)
-- ARGV[2] = refill-per-sec as integer milli-tokens-per-sec (refill * 1000)
-- ARGV[3] = now_ms (client unix-epoch milliseconds)
--
-- Return: { allowed (0|1), remaining_milli_tokens (integer) }.
--
-- Invariants:
--   * Atomic under EVAL — no race within the refill+take window.
--   * Clamp elapsed_ms to >= 0 to tolerate client wall-clock skew
--     (T-02-03-04: huge negative skew yields at most one extra grant).
--   * Both keys self-clean via EX 3600.
--   * remaining is reported in MILLI-tokens so Rust can compute Retry-After
--     without float round-trips.

local capacity_tokens    = tonumber(ARGV[1])
local refill_per_sec_mt  = tonumber(ARGV[2])   -- milli-tokens/sec
local now_ms             = tonumber(ARGV[3])
local capacity_mt        = capacity_tokens * 1000

-- Current bucket (milli-tokens). Default: full bucket.
local count_mt = tonumber(redis.call('GET', KEYS[1]))
if not count_mt then count_mt = capacity_mt end

-- Last refill timestamp. Default: now (no refill credit on first call).
local last_ms = tonumber(redis.call('GET', KEYS[2]))
if not last_ms then last_ms = now_ms end

-- Clamp to >= 0 (wall-clock skew tolerance).
local elapsed_ms = now_ms - last_ms
if elapsed_ms < 0 then elapsed_ms = 0 end

-- refill: (elapsed_ms / 1000) * refill_per_sec_mt  (in milli-tokens).
local refilled_mt = count_mt + math.floor(elapsed_ms * refill_per_sec_mt / 1000)
if refilled_mt > capacity_mt then refilled_mt = capacity_mt end

-- Check: need at least one full token (1000 milli-tokens).
if refilled_mt < 1000 then
    -- DENY. Persist refilled count + new timestamp so the next call sees
    -- the correct clock; do not decrement.
    redis.call('SET', KEYS[1], refilled_mt, 'EX', 3600)
    redis.call('SET', KEYS[2], now_ms,      'EX', 3600)
    return { 0, refilled_mt }
end

-- ALLOW: take one token.
local new_mt = refilled_mt - 1000
redis.call('SET', KEYS[1], new_mt,  'EX', 3600)
redis.call('SET', KEYS[2], now_ms,  'EX', 3600)
return { 1, new_mt }
