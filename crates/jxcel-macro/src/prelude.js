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

    function schemaApi(sheet, schema) {
      const column = (name) => find(schema.columns, name, "列");
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
          const v = values[k];
          cells[column(k).id] = v === undefined ? null : v;
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
        columns: schema.columns.map((c) => ({ name: c.name, type: c.type.kind, required: !!c.required })),
        rows() {
          return schema.rows.map(toRow);
        },
        get(id) {
          return toRow(row(id));
        },
        add(values) {
          const cells = toCells(values || {});
          const id = __newId();
          schema.rows.push({ id, cells: Object.assign({}, cells) });
          ops.push({ op: "add", sheet: sheet.id, schema: schema.id, row: id, cells });
          return id;
        },
        update(id, values) {
          const cells = toCells(values || {});
          const r = row(id);
          Object.assign(r.cells, cells);
          ops.push({ op: "update", sheet: sheet.id, schema: schema.id, row: id, cells });
        },
        remove(id) {
          row(id);
          schema.rows = schema.rows.filter((x) => x.id !== id);
          ops.push({ op: "remove", sheet: sheet.id, schema: schema.id, row: id });
        },
      };
    }

    const jx = {
      log,
      sheet(name) {
        const sheet = find(data.sheets, name, "シート");
        return {
          name: sheet.name,
          schema(schemaName) {
            return schemaApi(sheet, find(sheet.schemas, schemaName, "スキーマ"));
          },
        };
      },
    };

    return {
      jx,
      finish: (result) => JSON.stringify({ ops, logs, result: result === undefined ? null : result }),
    };
  };
})();
