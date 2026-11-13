// Node's Stats uses this order of operations, including normalized pre-epoch seconds.
const seconds = [-2, -1, 0, 1, 1_600_000_000, 1_754_539_513, 1_788_480_000];
const nanoseconds = [0, 100, 123400, 123456700, 944000000, 999999900];
process.stdout.write(JSON.stringify(seconds.flatMap(sec => nanoseconds.map(nsec => ({
  sec, nsec, milliseconds: sec * 1e3 + nsec / 1e6,
}))), null, 2) + "\n");
