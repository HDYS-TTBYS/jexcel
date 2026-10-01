// マクロ実行環境の JS 側。Rust から渡されたファイルの JSON を読み取り専用の台帳として持ち、
// マクロの読み書きを `jx` として提供する。書き込みは即座に台帳へ反映しつつ、操作ログ(ops)に記録する。
// 実際のファイルへの適用と型検証は、実行後に Rust 側が ops を再生して行う。
(function () {
  const fmt = (v) => (typeof v === "string" ? v : v === undefined ? "undefined" : JSON.stringify(v));

  globalThis.__makeState = function (data) {
    const logs = [];
    const ops = [];
    const log = (...a) => {
      logs.push(a.map(fmt).join(" "));
    };
    globalThis.console = { log, info: log, warn: log, error: log, debug: log };

    const find = (list, name, what) => {
      const x = list.find((s) => s.name === name);
      if (!x) throw new Error(what + "「" + name + "」が見つかりません");
      return x;
    };

    function schemaApi(sheet, schema, readOnly) {
      const column = (name) => find(schema.columns, name, "列");
      const noWrite = () => {
        throw new Error("計算式の中ではデータを書き換えられません");
      };
      const toRow = (r) => {
        const o = { _id: r.id };
        for (const c of schema.columns) o[c.name] = r.cells[c.id] === undefined ? null : r.cells[c.id];
        return o;
      };
      // 列名で渡された値を、列 ID をキーにしたセルへ変換する（未知の列名は即エラー）
      const toCells = (values) => {
        const cells = {};
        for (const k of Object.keys(values)) {
          if (k === "_id") continue;
          const c = column(k);
          // 計算列は式から決まる値なので、書き込めない
          if (c.computed) throw new Error("計算列「" + k + "」には書き込めません");
          const v = values[k];
          cells[c.id] = v === undefined ? null : v;
        }
        return cells;
      };
      const row = (id) => {
        const r = schema.rows.find((x) => x.id === id);
        if (!r) throw new Error("行 " + id + " が見つかりません");
        return r;
      };
      return {
        name: schema.name,
        columns: schema.columns.map((c) => ({ name: c.name, type: c.type.kind, required: !!c.required, computed: !!c.computed })),
        rows() {
          return schema.rows.map(toRow);
        },
        get(id) {
          return toRow(row(id));
        },
        add(values) {
          if (readOnly) noWrite();
          const cells = toCells(values || {});
          const id = __newId();
          schema.rows.push({ id, cells: Object.assign({}, cells) });
          ops.push({ op: "add", sheet: sheet.id, schema: schema.id, row: id, cells });
          return id;
        },
        update(id, values) {
          if (readOnly) noWrite();
          const cells = toCells(values || {});
          const r = row(id);
          Object.assign(r.cells, cells);
          ops.push({ op: "update", sheet: sheet.id, schema: schema.id, row: id, cells });
        },
        remove(id) {
          if (readOnly) noWrite();
          row(id);
          schema.rows = schema.rows.filter((x) => x.id !== id);
          ops.push({ op: "remove", sheet: sheet.id, schema: schema.id, row: id });
        },
      };
    }

    const makeJx = (readOnly) => ({
      log,
      sheet(name) {
        const sheet = find(data.sheets, name, "シート");
        return {
          name: sheet.name,
          schema(schemaName) {
            return schemaApi(sheet, find(sheet.schemas, schemaName, "スキーマ"), readOnly);
          },
        };
      },
    });
    const jx = makeJx(false);

    // 計算列を、表ごと・列ごとに列の並び順で評価する。結果は台帳（data）にも書き込むので、
    // 後ろの計算列や、同じ実行内のマクロ・他の表の計算式からも値が読める。
    // 失敗は行（セル）ごとに記録し、他のセルは計算を続ける。
    // 行オブジェクト（列名 → 値。_id は行 ID、計算列の値も入る）
    const rowObject = (schema, r) => {
      const o = { _id: r.id };
      for (const c of schema.columns) o[c.name] = r.cells[c.id] === undefined ? null : r.cells[c.id];
      return o;
    };

    // テンプレートの差し込み欄 `{{ 式 }}` を、表の全行について評価する。
    // 式は列名をそのまま変数として使える（with(row)）。row・jx（読み取り専用）・std も使える。
    // _no は 1 から始まる行番号。失敗は行・式ごとに隔離する。
    let exprsOut = [];
    function evalExprs(spec) {
      const sheet = data.sheets.find((s) => s.id === spec.sheet);
      const schema = sheet && sheet.schemas.find((s) => s.id === spec.schemaId);
      if (!schema) throw new Error("書き出し対象の表が見つかりません");
      const ro = makeJx(true);
      const fns = spec.exprs.map((src) => {
        try {
          // 間接 eval なので、モジュールの厳格モードではなく通常のスクリプトとして評価される（with が使える）
          return { f: (0, eval)("(function (row, jx, std) { with (row) { return (" + src + "\n); } })") };
        } catch (e) {
          return { e: String((e && e.message) || e) };
        }
      });
      exprsOut = schema.rows.map((r, i) => {
        const row = rowObject(schema, r);
        row._no = i + 1;
        return fns.map((fn) => {
          if (fn.e) return { e: fn.e };
          try {
            const v = fn.f(row, ro, globalThis.std);
            return { v: v === undefined ? null : v };
          } catch (e) {
            return { e: String((e && e.message) || e) };
          }
        });
      });
    }

    let computed = [];
    function computeAll(specs) {
      const ro = makeJx(true);
      const out = [];
      specs.forEach((spec, i) => {
        const sheet = data.sheets.find((s) => s.id === spec.sheet);
        const schema = sheet.schemas.find((s) => s.id === spec.schema);
        const col = schema.columns.find((c) => c.id === spec.column);
        const fn = globalThis.__fns && globalThis.__fns[i];
        // get(id) は行を探すので使わず、行オブジェクトを直接作る（全行で O(n²) になるのを避ける）
        const toRow = (r) => {
          const o = { _id: r.id };
          for (const c of schema.columns) o[c.name] = r.cells[c.id] === undefined ? null : r.cells[c.id];
          return o;
        };
        for (const r of schema.rows) {
          let res;
          if (spec.error || typeof fn !== "function") {
            res = { e: spec.error || "式が関数ではありません" };
          } else {
            try {
              const v = fn(toRow(r), ro);
              res = { v: v === undefined ? null : v };
            } catch (e) {
              res = { e: String((e && e.message) || e) };
            }
          }
          if (res.v === undefined || res.v === null) delete r.cells[col.id];
          else r.cells[col.id] = res.v;
          out.push(Object.assign({ schema: schema.id, column: col.id, row: r.id }, res));
        }
      });
      computed = out;
    }

    return {
      jx,
      computeAll,
      evalExprs,
      finish: (result) => JSON.stringify({ ops, logs, computed, exprs: exprsOut, result: result === undefined ? null : result }),
    };
  };
})();
