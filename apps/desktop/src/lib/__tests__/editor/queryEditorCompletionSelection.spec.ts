// @vitest-environment happy-dom

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { acceptCompletion, autocompletion, completionStatus, currentCompletions, selectedCompletionIndex, startCompletion } from "@codemirror/autocomplete";
import { EditorState, Transaction } from "@codemirror/state";
import { EditorView, keymap, runScopeHandlers } from "@codemirror/view";
import { describe, expect, it } from "vitest";

const queryEditorSource = readFileSync(resolve(process.cwd(), "apps/desktop/src/components/editor/QueryEditor.vue"), "utf8");

function createCompletionView(doc = "") {
  return new EditorView({
    parent: document.createElement("div"),
    state: EditorState.create({
      doc,
      selection: { anchor: doc.length },
      extensions: [
        autocompletion({
          activateOnTyping: true,
          activateOnTypingDelay: 0,
          interactionDelay: 0,
          selectOnOpen: false,
          override: [() => ({ from: 0, options: [{ label: "select" }, { label: "set" }] })],
        }),
        keymap.of([{ key: "Tab", run: acceptCompletion }]),
      ],
    }),
  });
}

function press(view: EditorView, key: string): boolean {
  return runScopeHandlers(view, new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }), "editor");
}

async function expectCompletionOpenWithoutSelection(view: EditorView) {
  await expect.poll(() => completionStatus(view.state)).toBe("active");
  expect(currentCompletions(view.state).map((completion) => completion.label)).toEqual(["select", "set"]);
  expect(selectedCompletionIndex(view.state)).toBeNull();
}

describe("QueryEditor completion selection", () => {
  it("binds SQL completion to CodeMirror's unselected-on-open mode", () => {
    expect(queryEditorSource).toMatch(/buildSqlCompletionExtension = \(\) =>\s+autocompletion\(\{\s+activateOnTyping: true,\s+selectOnOpen: false,/);
  });

  it("opens manual completion unselected until ArrowDown selects the first option", async () => {
    const view = createCompletionView();

    expect(startCompletion(view)).toBe(true);
    await expectCompletionOpenWithoutSelection(view);
    expect(press(view, "Tab")).toBe(false);
    expect(view.state.doc.toString()).toBe("");

    expect(press(view, "ArrowDown")).toBe(true);
    expect(selectedCompletionIndex(view.state)).toBe(0);
    expect(press(view, "Tab")).toBe(true);
    expect(view.state.doc.toString()).toBe("select");
    view.destroy();
  });

  it("opens typing-triggered completion without selecting the first option", async () => {
    const view = createCompletionView();

    view.dispatch({
      changes: { from: 0, insert: "s" },
      annotations: Transaction.userEvent.of("input.type"),
    });
    await expectCompletionOpenWithoutSelection(view);
    expect(press(view, "Tab")).toBe(false);
    expect(view.state.doc.toString()).toBe("s");
    view.destroy();
  });
});
