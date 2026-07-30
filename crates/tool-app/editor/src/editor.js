import { basicSetup, EditorView } from "codemirror";
import { json, jsonParseLinter } from "@codemirror/lang-json";
import { linter } from "@codemirror/lint";

const editors = new Map();

function syncTextarea(textarea, value) {
  textarea.value = value;
  textarea.dispatchEvent(new Event("input", { bubbles: true }));
}

window.CocosBuildLanEditor = {
  mount(hostId, textareaId, value, dark) {
    this.destroy(hostId);
    const host = document.getElementById(hostId);
    const textarea = document.getElementById(textareaId);
    if (!host || !textarea) return;
    const view = new EditorView({
      parent: host,
      doc: value || "{}",
      extensions: [
        basicSetup,
        json(),
        linter(jsonParseLinter()),
        EditorView.lineWrapping,
        EditorView.theme({
          "&": {
            height: "260px",
            color: dark ? "#e5e7eb" : "#172033",
            backgroundColor: dark ? "#111827" : "#ffffff"
          },
          ".cm-content": { fontFamily: "Consolas, monospace", fontSize: "12px" },
          ".cm-gutters": {
            backgroundColor: dark ? "#182235" : "#f5f7fa",
            color: dark ? "#94a3b8" : "#667085",
            border: "0"
          },
          ".cm-scroller": { overflow: "auto" }
        }, { dark }),
        EditorView.updateListener.of((update) => {
          if (update.docChanged) syncTextarea(textarea, update.state.doc.toString());
        })
      ]
    });
    editors.set(hostId, view);
  },
  destroy(hostId) {
    const view = editors.get(hostId);
    if (view) view.destroy();
    editors.delete(hostId);
  },
  format(hostId) {
    const view = editors.get(hostId);
    if (!view) return false;
    try {
      const formatted = JSON.stringify(JSON.parse(view.state.doc.toString()), null, 2);
      view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: formatted } });
      return true;
    } catch (_) {
      return false;
    }
  }
};
