// 標準ライブラリ `std`。マクロと計算列の式のどちらからも、import なしで使える。
// 型と説明は ui/src/jxcelApi.d.ts.txt（エディタの補完・ホバー用）。関数を足したら両方に書くこと
// （Rust のテストが「型定義にある関数が実在する」ことと「実在する関数が型定義にある」ことを検査する）。
//
// 方針: 値が無い(null/undefined)ものは「無視」か「null を返す」。不正な値（数値でない文字列など）は
// 黙って 0 にせず例外にする（計算列では #ERROR になり、原因が分かる）。
(function () {
  // ---- 共通 ----
  const isNil = (v) => v === null || v === undefined;

  /** 数値として使える値だけを取り出す。null は無視、数値でないものは例外。 */
  function nums(values) {
    const out = [];
    for (const v of values) {
      if (isNil(v)) continue;
      const n = typeof v === "number" ? v : typeof v === "string" && v.trim() !== "" ? Number(v) : NaN;
      if (Number.isNaN(n)) throw new Error("数値ではありません: " + JSON.stringify(v));
      out.push(n);
    }
    return out;
  }

  function round(x, digits) {
    if (isNil(x)) return null;
    const d = digits === undefined ? 0 : digits;
    const n = nums([x])[0];
    const sign = n < 0 ? -1 : 1;
    const abs = Math.abs(n);
    // 1.005 のような誤差を避けるため、指数表記で桁をずらしてから丸める（0.5 は常に遠ざかる方向へ）
    const shifted = String(abs).includes("e") ? abs * Math.pow(10, d) : Number(abs + "e" + d);
    const r = Math.round(shifted);
    return sign * (String(r).includes("e") ? r / Math.pow(10, d) : Number(r + "e" + -d));
  }
  function directed(x, digits, fn) {
    if (isNil(x)) return null;
    const d = digits === undefined ? 0 : digits;
    const n = nums([x])[0];
    const shifted = String(n).includes("e") ? n * Math.pow(10, d) : Number(n + "e" + d);
    const r = fn(shifted);
    return String(r).includes("e") ? r / Math.pow(10, d) : Number(r + "e" + -d);
  }

  const cell = (row, col) => {
    if (!(col in row)) throw new Error("列「" + col + "」がありません");
    return row[col];
  };
  const keyOf = (v) => JSON.stringify(v === undefined ? null : v);

  // ---- 日付（"YYYY-MM-DD" の文字列で入出力する。日時型の値は日付部分だけを使う） ----
  const pad2 = (n) => (n < 10 ? "0" : "") + n;
  // 1970-01-01 からの日数 <-> 年月日（タイムゾーンに依存しない暦計算）
  function toDays(y, m, d) {
    const yy = y - (m <= 2 ? 1 : 0);
    const era = Math.floor(yy / 400);
    const yoe = yy - era * 400;
    const doy = Math.floor((153 * (m + (m > 2 ? -3 : 9)) + 2) / 5) + d - 1;
    const doe = yoe * 365 + Math.floor(yoe / 4) - Math.floor(yoe / 100) + doy;
    return era * 146097 + doe - 719468;
  }
  function fromDays(z0) {
    const z = z0 + 719468;
    const era = Math.floor(z / 146097);
    const doe = z - era * 146097;
    const yoe = Math.floor((doe - Math.floor(doe / 1460) + Math.floor(doe / 36524) - Math.floor(doe / 146096)) / 365);
    const doy = doe - (365 * yoe + Math.floor(yoe / 4) - Math.floor(yoe / 100));
    const mp = Math.floor((5 * doy + 2) / 153);
    const d = doy - Math.floor((153 * mp + 2) / 5) + 1;
    const m = mp + (mp < 10 ? 3 : -9);
    return [yoe + era * 400 + (m <= 2 ? 1 : 0), m, d];
  }
  const daysIn = (y, m) => toDays(m === 12 ? y + 1 : y, m === 12 ? 1 : m + 1, 1) - toDays(y, m, 1);
  function parseDate(s) {
    if (typeof s !== "string") throw new Error("日付ではありません: " + JSON.stringify(s));
    const m = /^(\d{4})-(\d{2})-(\d{2})(?:$|[Tt ])/.exec(s);
    if (!m) throw new Error("日付ではありません: " + JSON.stringify(s));
    const [y, mo, d] = [Number(m[1]), Number(m[2]), Number(m[3])];
    if (mo < 1 || mo > 12 || d < 1 || d > daysIn(y, mo)) throw new Error("存在しない日付です: " + s);
    return [y, mo, d];
  }
  const fmtDate = (y, m, d) => String(y).padStart(4, "0") + "-" + pad2(m) + "-" + pad2(d);
  const dayNum = (s) => toDays(...parseDate(s));
  const dateOf = (z) => fmtDate(...fromDays(z));
  const WEEK = ["日", "月", "火", "水", "木", "金", "土"];
  const weekday = (s) => (((dayNum(s) + 4) % 7) + 7) % 7; // 1970-01-01 は木曜
  // 元号（開始日の新しい順）。明治より前は扱わない。
  const ERAS = [
    ["令和", 2019, 5, 1],
    ["平成", 1989, 1, 8],
    ["昭和", 1926, 12, 25],
    ["大正", 1912, 7, 30],
    ["明治", 1868, 1, 25],
  ];

  // 日時 "YYYY-MM-DDTHH:MM:SS[.fff][Z|±HH:MM]"（オフセットは省略可）。時刻は書かれたままで、タイムゾーンの変換はしない
  function parseDateTime(s) {
    const [y, mo, d] = parseDate(s);
    const m = /^\d{4}-\d{2}-\d{2}[Tt ](\d{2}):(\d{2})(?::(\d{2}))?(\.\d+)?(Z|z|[+-]\d{2}:\d{2})?$/.exec(s);
    if (!m) throw new Error("日時ではありません: " + JSON.stringify(s));
    const [hh, mi, ss] = [Number(m[1]), Number(m[2]), Number(m[3] || 0)];
    if (hh > 23 || mi > 59 || ss > 59) throw new Error("存在しない時刻です: " + s);
    let off = null;
    if (m[5]) off = /^[Zz]$/.test(m[5]) ? 0 : (m[5][0] === "-" ? -1 : 1) * (Number(m[5].slice(1, 3)) * 60 + Number(m[5].slice(4, 6)));
    return { y, mo, d, hh, mi, ss, frac: m[4] || "", off };
  }
  // オフセット（分）の引数: 数値（分）・"Z"・"+09:00"
  function offsetArg(o) {
    if (typeof o === "number" && Number.isInteger(o) && Math.abs(o) < 24 * 60) return o;
    if (typeof o === "string") {
      if (/^[Zz]$/.test(o)) return 0;
      const m = /^([+-])(\d{2}):(\d{2})$/.exec(o);
      if (m && Number(m[2]) < 24 && Number(m[3]) < 60) return (m[1] === "-" ? -1 : 1) * (Number(m[2]) * 60 + Number(m[3]));
    }
    throw new Error("オフセットは \"+09:00\"・\"Z\"・分（540 など）で指定してください: " + JSON.stringify(o));
  }
  function fmtOffset(min, z) {
    if (min === 0 && z) return "Z";
    const a = Math.abs(min);
    return (min < 0 ? "-" : "+") + pad2(Math.floor(a / 60)) + ":" + pad2(a % 60);
  }

  // 日時の「瞬間」（オフセットを考慮した UTC の秒と、小数秒の 9 桁）。オフセットのない日時は比べられないので例外
  function instantOf(s) {
    const t = parseDateTime(s);
    if (t.off === null) throw new Error("オフセットのない日時は比較できません: " + s);
    return { sec: toDays(t.y, t.mo, t.d) * 86400 + t.hh * 3600 + t.mi * 60 + t.ss - t.off * 60, frac: (t.frac.slice(1) + "000000000").slice(0, 9) };
  }
  const cmpInstant = (a, b) => (a.sec !== b.sec ? (a.sec < b.sec ? -1 : 1) : a.frac < b.frac ? -1 : a.frac > b.frac ? 1 : 0);
  // オフセット付きの日時の文字列か（sortBy が、文字列の比較ではなく瞬間で比べるかの判定）
  const DT_OFFSET = /^\d{4}-\d{2}-\d{2}[Tt]\d{2}:\d{2}:\d{2}(\.\d+)?([Zz]|[+-]\d{2}:\d{2})$/;

  function pickInstant(values, dir) {
    let best = null;
    let bestKey = null;
    for (const v of values) {
      if (isNil(v)) continue;
      const k = instantOf(v);
      if (best === null || cmpInstant(k, bestKey) * dir > 0) [best, bestKey] = [v, k];
    }
    return best;
  }
  const date = {
    today() {
      const t = new Date();
      return fmtDate(t.getFullYear(), t.getMonth() + 1, t.getDate());
    },
    isValid(s) {
      try {
        parseDate(s);
        return true;
      } catch (e) {
        return false;
      }
    },
    addDays(s, n) {
      return isNil(s) ? null : dateOf(dayNum(s) + n);
    },
    // 月末を超える日は月末に丸める（1/31 + 1 か月 = 2/28 または 2/29）
    addMonths(s, n) {
      if (isNil(s)) return null;
      const [y, m, d] = parseDate(s);
      const total = y * 12 + (m - 1) + n;
      const ny = Math.floor(total / 12);
      const nm = (((total % 12) + 12) % 12) + 1;
      return fmtDate(ny, nm, Math.min(d, daysIn(ny, nm)));
    },
    addYears(s, n) {
      return date.addMonths(s, n * 12);
    },
    /** a - b（日数） */
    diffDays(a, b) {
      return isNil(a) || isNil(b) ? null : dayNum(a) - dayNum(b);
    },
    startOfMonth(s) {
      if (isNil(s)) return null;
      const [y, m] = parseDate(s);
      return fmtDate(y, m, 1);
    },
    endOfMonth(s) {
      if (isNil(s)) return null;
      const [y, m] = parseDate(s);
      return fmtDate(y, m, daysIn(y, m));
    },
    /** 0=日 〜 6=土 */
    weekday(s) {
      return isNil(s) ? null : weekday(s);
    },
    isWeekend(s) {
      if (isNil(s)) return null;
      const w = weekday(s);
      return w === 0 || w === 6;
    },
    /** 土日と holidays（"YYYY-MM-DD" の配列）を除いて n 営業日後（負なら前）の日付 */
    addWorkdays(s, n, holidays) {
      if (isNil(s)) return null;
      const off = new Set(holidays || []);
      let z = dayNum(s);
      const step = n < 0 ? -1 : 1;
      for (let left = Math.abs(n); left > 0; ) {
        z += step;
        const w = (((z + 4) % 7) + 7) % 7;
        if (w !== 0 && w !== 6 && !off.has(dateOf(z))) left--;
      }
      return dateOf(z);
    },
    /** 始点と終点を含む営業日数 */
    workdaysBetween(a, b, holidays) {
      if (isNil(a) || isNil(b)) return null;
      const off = new Set(holidays || []);
      let [lo, hi] = [dayNum(a), dayNum(b)];
      const sign = lo <= hi ? 1 : -1;
      if (sign < 0) [lo, hi] = [hi, lo];
      let count = 0;
      for (let z = lo; z <= hi; z++) {
        const w = (((z + 4) % 7) + 7) % 7;
        if (w !== 0 && w !== 6 && !off.has(dateOf(z))) count++;
      }
      return sign * count;
    },
    /** 書式: YYYY YY MM M DD D ddd（曜日 1 文字）、日時なら HH mm ss。例: "YYYY年M月D日(ddd) HH:mm" */
    format(s, pattern) {
      if (isNil(s)) return null;
      const [y, m, d] = parseDate(s);
      const map = { YYYY: String(y).padStart(4, "0"), YY: String(y).slice(-2), MM: pad2(m), M: String(m), DD: pad2(d), D: String(d), ddd: WEEK[weekday(s)] };
      if (/HH|mm|ss/.test(pattern)) {
        const t = parseDateTime(s);
        Object.assign(map, { HH: pad2(t.hh), mm: pad2(t.mi), ss: pad2(t.ss) });
      }
      return pattern.replace(/YYYY|YY|MM|M|DD|D|ddd|HH|mm|ss/g, (t) => map[t]);
    },
    /** 現在の日時（実行した環境のタイムゾーンのオフセット付き。例: "2024-01-31T10:30:00+09:00"） */
    now() {
      const t = new Date();
      const off = -t.getTimezoneOffset();
      return fmtDate(t.getFullYear(), t.getMonth() + 1, t.getDate()) + "T" + pad2(t.getHours()) + ":" + pad2(t.getMinutes()) + ":" + pad2(t.getSeconds()) + fmtOffset(off, false);
    },
    /** 日時のオフセット（分。"Z" と "+00:00" は 0、東京は 540）。オフセットのない日時は null */
    offsetMinutes(s) {
      return isNil(s) ? null : parseDateTime(s).off;
    },
    /** a と b の前後（瞬間で比べる）: a が前なら -1、同じなら 0、後なら 1。どちらかが null なら null。オフセットのない日時は例外 */
    compare(a, b) {
      return isNil(a) || isNil(b) ? null : cmpInstant(instantOf(a), instantOf(b));
    },
    /** 1970-01-01T00:00:00Z からのミリ秒（オフセットを考慮）。オフセットのない日時は例外 */
    toEpochMs(s) {
      if (isNil(s)) return null;
      const t = instantOf(s);
      return t.sec * 1000 + Number(t.frac.slice(0, 3));
    },
    /** a - b の秒数（オフセットを考慮。小数秒も含む）。どちらかが null なら null */
    diffSeconds(a, b) {
      if (isNil(a) || isNil(b)) return null;
      const [x, y] = [instantOf(a), instantOf(b)];
      return x.sec - y.sec + (Number(x.frac) - Number(y.frac)) / 1e9;
    },
    /** 一番早い日時（瞬間で比べ、元の文字列を返す）。null は無視し、残りが無ければ null */
    earliest(values) {
      return pickInstant(values, -1);
    },
    /** 一番遅い日時（瞬間で比べ、元の文字列を返す）。null は無視し、残りが無ければ null */
    latest(values) {
      return pickInstant(values, 1);
    },
    /** 同じ時刻を、別のタイムゾーンのオフセットで表した日時にする。例: toOffset("2024-01-31T01:30:00Z", "+09:00") → "2024-01-31T10:30:00+09:00"。元にオフセットがない日時は変換できず例外 */
    toOffset(s, offset) {
      if (isNil(s)) return null;
      const t = parseDateTime(s);
      const to = offsetArg(offset);
      if (t.off === null) throw new Error("オフセットのない日時は変換できません: " + s);
      const secs = toDays(t.y, t.mo, t.d) * 86400 + t.hh * 3600 + t.mi * 60 + t.ss - t.off * 60 + to * 60;
      const z = Math.floor(secs / 86400);
      const r = secs - z * 86400;
      const zulu = typeof offset === "string" && /^[Zz]$/.test(offset);
      return dateOf(z) + "T" + pad2(Math.floor(r / 3600)) + ":" + pad2(Math.floor((r % 3600) / 60)) + ":" + pad2(r % 60) + t.frac + fmtOffset(to, zulu);
    },
    /** 和暦。例: "令和6年1月31日"（1 年は「元年」） */
    toWareki(s) {
      if (isNil(s)) return null;
      const [y, m, d] = parseDate(s);
      const z = toDays(y, m, d);
      for (const [name, ey, em, ed] of ERAS) {
        if (z >= toDays(ey, em, ed)) {
          const year = y - ey + 1;
          return name + (year === 1 ? "元" : year) + "年" + m + "月" + d + "日";
        }
      }
      throw new Error("和暦に変換できない日付です（明治より前）: " + s);
    },
  };

  // ---- 10 進数（Decimal 型の値は文字列。BigInt で誤差なく計算する） ----
  function parseDec(v) {
    const s = typeof v === "number" ? (String(v).includes("e") ? v.toFixed(20).replace(/0+$/, "").replace(/\.$/, "") : String(v)) : v;
    const m = typeof s === "string" ? /^(-?)(\d+)(?:\.(\d+))?$/.exec(s.trim()) : null;
    if (!m) throw new Error("10進数ではありません: " + JSON.stringify(v));
    const frac = m[3] || "";
    return { v: (m[1] ? -1n : 1n) * BigInt(m[2] + frac), s: frac.length };
  }
  const pow10 = (n) => 10n ** BigInt(n);
  const align = (a, b) => {
    const s = Math.max(a.s, b.s);
    return [a.v * pow10(s - a.s), b.v * pow10(s - b.s), s];
  };
  function showDec(v, s) {
    const neg = v < 0n;
    let digits = (neg ? -v : v).toString();
    if (s > 0) {
      digits = digits.padStart(s + 1, "0");
      digits = digits.slice(0, -s) + "." + digits.slice(-s);
    }
    return (neg && /[1-9]/.test(digits) ? "-" : "") + digits;
  }
  /** 小数点以下 to 桁へ。四捨五入（0.5 は 0 から遠ざかる方向） */
  function rescale(x, to) {
    if (to >= x.s) return { v: x.v * pow10(to - x.s), s: to };
    const div = pow10(x.s - to);
    let q = x.v / div;
    const r = x.v % div;
    if ((r < 0n ? -r : r) * 2n >= div) q += x.v < 0n ? -1n : 1n;
    return { v: q, s: to };
  }
  const dec = {
    add(a, b) {
      const [x, y, s] = align(parseDec(a), parseDec(b));
      return showDec(x + y, s);
    },
    sub(a, b) {
      const [x, y, s] = align(parseDec(a), parseDec(b));
      return showDec(x - y, s);
    },
    mul(a, b) {
      const [x, y] = [parseDec(a), parseDec(b)];
      return showDec(x.v * y.v, x.s + y.s);
    },
    /** a / b を小数点以下 scale 桁で（四捨五入） */
    div(a, b, scale) {
      const [x, y] = [parseDec(a), parseDec(b)];
      if (y.v === 0n) throw new Error("0 で割れません");
      const num = x.v * pow10(scale + y.s);
      const den = y.v * pow10(x.s);
      let q = num / den;
      const r = num % den;
      if ((r < 0n ? -r : r) * 2n >= (den < 0n ? -den : den)) q += (num < 0n) !== (den < 0n) ? -1n : 1n;
      return showDec(q, scale);
    },
    round(a, scale) {
      if (isNil(a)) return null;
      const r = rescale(parseDec(a), scale === undefined ? 0 : scale);
      return showDec(r.v, r.s);
    },
    /** 小数点以下 scale 桁に揃えた文字列（"1.5" → "1.50"） */
    fixed(a, scale) {
      return dec.round(a, scale);
    },
    cmp(a, b) {
      const [x, y] = align(parseDec(a), parseDec(b));
      return x < y ? -1 : x > y ? 1 : 0;
    },
    sum(values) {
      let acc = { v: 0n, s: 0 };
      for (const v of values) {
        if (isNil(v)) continue;
        const [x, y, s] = align(acc, parseDec(v));
        acc = { v: x + y, s };
      }
      return showDec(acc.v, acc.s);
    },
    toNumber(a) {
      return isNil(a) ? null : Number(a);
    },
  };

  // ---- 文字列 ----
  const text = {
    isBlank: (v) => isNil(v) || (typeof v === "string" && v.replace(/[\s　]/g, "") === ""),
    /** 前後の空白（全角スペース含む）を除く */
    trim: (s) => (isNil(s) ? null : String(s).replace(/^[\s　]+|[\s　]+$/g, "")),
    /** 全角英数・記号・スペースを半角に */
    toHalfWidth: (s) =>
      isNil(s)
        ? null
        : String(s)
            .replace(/[！-～]/g, (c) => String.fromCharCode(c.charCodeAt(0) - 0xfee0))
            .replace(/　/g, " "),
    /** 半角英数・記号・スペースを全角に。半角カナも全角カナに */
    toFullWidth: (s) =>
      isNil(s)
        ? null
        : String(s)
            .replace(/[｡-ﾟ]+/g, (m) => m.normalize("NFKC"))
            .replace(/[!-~]/g, (c) => String.fromCharCode(c.charCodeAt(0) + 0xfee0))
            .replace(/ /g, "　"),
    toKatakana: (s) => (isNil(s) ? null : String(s).replace(/[ぁ-ゖ]/g, (c) => String.fromCharCode(c.charCodeAt(0) + 0x60))),
    toHiragana: (s) => (isNil(s) ? null : String(s).replace(/[ァ-ヶ]/g, (c) => String.fromCharCode(c.charCodeAt(0) - 0x60))),
    /** 表記ゆれの整理: 全角英数・半角カナを正規化(NFKC)し、前後の空白を除き、連続する空白を 1 つに */
    normalize: (s) => (isNil(s) ? null : String(s).normalize("NFKC").replace(/[\s　]+/g, " ").trim()),
    zeroPad: (n, width) => String(n).padStart(width, "0"),
    /** 3 桁区切り。例: formatNumber(1234567.891, 1) → "1,234,567.9" */
    formatNumber(n, digits) {
      if (isNil(n)) return null;
      const d = digits === undefined ? 0 : digits;
      const fixed = dec.round(String(round(n, d)), d);
      const [int, frac] = fixed.split(".");
      const neg = int.startsWith("-");
      const grouped = (neg ? int.slice(1) : int).replace(/\B(?=(\d{3})+(?!\d))/g, ",");
      return (neg ? "-" : "") + grouped + (frac ? "." + frac : "");
    },
    yen: (n) => (isNil(n) ? null : (n < 0 ? "-" : "") + "¥" + text.formatNumber(Math.abs(n), 0)),
  };

  const std = {
    // 集計（null は無視。数値でないものは例外）
    sum: (values) => nums(values).reduce((a, b) => a + b, 0),
    avg(values) {
      const n = nums(values);
      return n.length === 0 ? null : n.reduce((a, b) => a + b, 0) / n.length;
    },
    min(values) {
      const n = nums(values);
      return n.length === 0 ? null : Math.min(...n);
    },
    max(values) {
      const n = nums(values);
      return n.length === 0 ? null : Math.max(...n);
    },
    median(values) {
      const n = nums(values).sort((a, b) => a - b);
      if (n.length === 0) return null;
      const mid = n.length >> 1;
      return n.length % 2 ? n[mid] : (n[mid - 1] + n[mid]) / 2;
    },
    /** null でない値の個数 */
    count: (values) => values.filter((v) => !isNil(v)).length,
    round,
    floor: (x, digits) => directed(x, digits, Math.floor),
    ceil: (x, digits) => directed(x, digits, Math.ceil),
    clamp: (x, lo, hi) => (isNil(x) ? null : Math.min(Math.max(nums([x])[0], lo), hi)),
    coalesce(...values) {
      for (const v of values) if (!isNil(v)) return v;
      return null;
    },

    // 表（rows() の結果）の操作。列は名前で指定
    pluck: (rows, col) => rows.map((r) => cell(r, col)),
    sumBy: (rows, col) => std.sum(rows.map((r) => cell(r, col))),
    avgBy: (rows, col) => std.avg(rows.map((r) => cell(r, col))),
    countBy: (rows, col) => std.count(rows.map((r) => cell(r, col))),
    where: (rows, col, value) => rows.filter((r) => keyOf(cell(r, col)) === keyOf(value)),
    find(rows, col, value) {
      const k = keyOf(value);
      return rows.find((r) => keyOf(cell(r, col)) === k) || null;
    },
    /** 見つからなければ fallback（既定 null）。VLOOKUP / XLOOKUP 相当 */
    lookup(rows, keyCol, key, valueCol, fallback) {
      const r = std.find(rows, keyCol, key);
      return r ? cell(r, valueCol) : fallback === undefined ? null : fallback;
    },
    /** 列の値ごとにまとめる。出現順を保つ */
    groupBy(rows, col) {
      const groups = new Map();
      for (const r of rows) {
        const key = cell(r, col);
        const k = keyOf(key);
        if (!groups.has(k)) groups.set(k, { key, rows: [] });
        groups.get(k).rows.push(r);
      }
      return [...groups.values()];
    },
    uniq(values) {
      const seen = new Set();
      return values.filter((v) => {
        const k = keyOf(v);
        if (seen.has(k)) return false;
        seen.add(k);
        return true;
      });
    },
    /** 新しい配列を返す。空の値は昇順・降順どちらでも最後。オフセット付きの日時の文字列は、文字列ではなく瞬間で比べる */
    sortBy(rows, col, order) {
      const dir = order === "desc" ? -1 : 1;
      return rows
        .map((r, i) => [r, i])
        .sort(([a, i], [b, j]) => {
          const [x, y] = [cell(a, col), cell(b, col)];
          if (isNil(x) || isNil(y)) return isNil(x) && isNil(y) ? i - j : isNil(x) ? 1 : -1;
          // オフセット付きの日時どうしは、文字列ではなく瞬間で比べる（+09:00 と Z が混ざっていても正しく並ぶ）
          if (typeof x === "string" && typeof y === "string" && DT_OFFSET.test(x) && DT_OFFSET.test(y)) {
            try {
              const c = cmpInstant(instantOf(x), instantOf(y));
              return c !== 0 ? c * dir : i - j;
            } catch (e) {
              // 存在しない日時などは、文字列の比較にする
            }
          }
          return x < y ? -dir : x > y ? dir : i - j;
        })
        .map(([r]) => r);
    },

    date,
    dec,
    text,
  };

  globalThis.std = Object.freeze(Object.assign(std, { date: Object.freeze(date), dec: Object.freeze(dec), text: Object.freeze(text) }));
})();
