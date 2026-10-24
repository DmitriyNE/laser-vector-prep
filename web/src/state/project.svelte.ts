// The UI's view of the project. The worker owns the geometry; this store owns UI state and
// an editable mirror of the profile, pushes whole-profile updates, and keeps only the newest
// evaluation (older results can never overwrite a newer one).

import type {
  EvaluationSummary,
  Diagnostic,
  MatchRule,
  Preview,
  Profile,
  SourceSummary,
} from "../core/types.js";
import { GeometryWorker, WorkerCrashed } from "../worker/client.js";
import { download, shortId } from "../util/download.js";
import * as db from "./profileStore.js";
import type { SavedProfile } from "./profileStore.js";

export type Selection =
  | { kind: "none" }
  | { kind: "source"; file: number; layer: string }
  | { kind: "binding"; id: string }
  | { kind: "layer"; id: string }
  | { kind: "pass"; id: string }
  | { kind: "group"; id: string };

export const PREVIEW_TOLERANCE_MM = 0.005;
const LAST_PROFILE_KEY = "lastProfile";

export class ProjectStore {
  worker = new GeometryWorker();

  ready = $state(false);
  coreVersion = $state("…");
  threads: number | null = $state(null);
  error: string | null = $state(null);

  sources: SourceSummary[] = $state([]);
  presets: Profile[] = $state([]);
  /** File extensions the core can read (lower case, no dot). */
  acceptedExtensions: string[] = $state(["dxf"]);
  saved: SavedProfile[] = $state([]);
  profile: Profile = $state({
    schemaVersion: 1,
    profileId: "empty",
    name: "Empty",
    sourceBindings: [],
    workingLayers: [],
    outputGroups: [],
    export: { strokeWidthMm: 0.0254, marginMm: 5, separateFiles: false, flattenToleranceMm: 0.001 },
    optimizer: { mode: "asGenerated" },
  });
  /** Saved profile the edits are based on; `null` while based on a preset or an import. */
  activeSavedId: string | null = $state(null);
  /** Preset the current profile derives from (for Reset) when not based on a saved one. */
  basePresetId: string | null = $state(null);
  dirty = $state(false);

  evaluation: EvaluationSummary | null = $state(null);
  evaluating = $state(false);
  /** Bumps whenever preview geometry may have changed. */
  previewRevision = $state(0);

  selection: Selection = $state({ kind: "none" });
  hiddenSourceLayers = $state(new Set<string>());
  hiddenLayers = $state(new Set<string>());
  hiddenPasses = $state(new Set<string>());
  showJumps = $state(false);
  showSources = $state(true);
  showLayers = $state(true);

  #evalToken = 0;
  #debounce: ReturnType<typeof setTimeout> | undefined;
  /** Imported files, kept (in memory only) so sources can be re-imported after a crash. */
  #files: File[] = [];

  async init() {
    this.worker.onCrash = (msg) => void this.recover(msg);
    try {
      this.coreVersion = await this.worker.call({ kind: "version" });
      this.threads = await this.worker.call({ kind: "threads" });
      this.presets = await this.worker.call({ kind: "presets" });
      this.acceptedExtensions = await this.worker.call({ kind: "acceptedExtensions" });
      this.saved = await db.listProfiles();
      const lastId = await db.getSetting<string>(LAST_PROFILE_KEY);
      const last = this.saved.find((s) => s.id === lastId);
      if (last) {
        this.profile = await this.#loadJson(last.json);
        this.activeSavedId = last.id;
      } else {
        this.profile = await this.worker.call({ kind: "getProfile" });
        this.basePresetId = this.profile.profileId;
      }
      this.ready = true;
      await this.evaluate();
    } catch (e) {
      this.error = e instanceof Error ? e.message : String(e);
    }
  }

  // ---- sources -----------------------------------------------------------------------

  async addFiles(files: Iterable<File>) {
    for (const f of files) {
      try {
        const bytes = new Uint8Array(await f.arrayBuffer());
        const summary = await this.worker.call({ kind: "addSource", name: f.name, bytes });
        this.sources = [...this.sources, summary];
        this.#files.push(f);
      } catch (e) {
        this.error = `${f.name}: ${e instanceof Error ? e.message : String(e)}`;
      }
    }
    await this.evaluate();
  }

  async removeSource(index: number) {
    await this.worker.call({ kind: "removeSource", index });
    this.#files.splice(index, 1);
    this.sources = await this.worker.call({ kind: "sources" });
    if (this.selection.kind === "source") this.selection = { kind: "none" };
    await this.evaluate();
  }

  /** The wasm instance trapped: a fresh worker is up; re-import the files and re-evaluate. */
  async recover(message: string) {
    this.error = `Geometry worker crashed (${message}); restarted and re-imported sources.`;
    const files = this.#files;
    this.#files = [];
    this.sources = [];
    this.evaluation = null;
    for (const f of files) {
      try {
        const bytes = new Uint8Array(await f.arrayBuffer());
        this.sources = [...this.sources, await this.worker.call({ kind: "addSource", name: f.name, bytes })];
        this.#files.push(f);
      } catch {
        // keep going; the diagnostics panel shows what is missing
      }
    }
    await this.evaluate();
  }

  // ---- profile edits -----------------------------------------------------------------

  /** Mutate the profile in place; evaluation follows after a short debounce. */
  update(mutate: (p: Profile) => void) {
    mutate(this.profile);
    this.dirty = true;
    this.scheduleEvaluate();
  }

  scheduleEvaluate() {
    clearTimeout(this.#debounce);
    this.#debounce = setTimeout(() => void this.evaluate(), 150);
  }

  async evaluate() {
    const token = ++this.#evalToken;
    this.evaluating = true;
    try {
      await this.worker.call({ kind: "setProfile", profile: $state.snapshot(this.profile) });
      const ev = await this.worker.call({ kind: "evaluate" });
      if (token !== this.#evalToken) return; // superseded
      this.evaluation = ev;
      this.previewRevision++;
      this.error = null;
    } catch (e) {
      if (token === this.#evalToken && !(e instanceof WorkerCrashed)) {
        this.error = e instanceof Error ? e.message : String(e);
      }
    } finally {
      if (token === this.#evalToken) this.evaluating = false;
    }
  }

  preview(kind: "pass" | "layer", id: string): Promise<Preview | null> {
    return kind === "pass"
      ? this.worker.call({ kind: "previewPass", id, toleranceMm: PREVIEW_TOLERANCE_MM })
      : this.worker.call({ kind: "previewLayer", id, toleranceMm: PREVIEW_TOLERANCE_MM });
  }

  previewSource(file: number, layer: string): Promise<Preview | null> {
    return this.worker.call({ kind: "previewSource", file, layer, toleranceMm: PREVIEW_TOLERANCE_MM });
  }

  previewJumps(id: string): Promise<Preview | null> {
    return this.worker.call({ kind: "previewJumps", id });
  }

  // ---- profiles ----------------------------------------------------------------------

  applyPreset(presetId: string) {
    const p = this.presets.find((x) => x.profileId === presetId);
    if (!p) return;
    // Presets are reactive state; a plain snapshot is needed before cloning.
    this.profile = structuredClone($state.snapshot(p)) as Profile;
    this.activeSavedId = null;
    this.basePresetId = presetId;
    this.dirty = false;
    void db.setSetting(LAST_PROFILE_KEY, null);
    void this.evaluate();
  }

  async applySaved(id: string) {
    const s = this.saved.find((x) => x.id === id);
    if (!s) return;
    try {
      this.profile = await this.#loadJson(s.json);
    } catch (e) {
      this.error = `${s.name}: ${e instanceof Error ? e.message : String(e)}`;
      return;
    }
    this.activeSavedId = id;
    this.basePresetId = null;
    this.dirty = false;
    void db.setSetting(LAST_PROFILE_KEY, id);
    void this.evaluate();
  }

  /** Push stored JSON through the worker so it is validated (and, later, migrated). */
  async #loadJson(json: string): Promise<Profile> {
    await this.worker.call({ kind: "setProfile", profile: JSON.parse(json) as Profile });
    return this.worker.call({ kind: "getProfile" });
  }

  /** Discard edits: back to the saved record or preset the profile came from. */
  reset() {
    if (this.activeSavedId) this.applySaved(this.activeSavedId);
    else if (this.basePresetId) this.applyPreset(this.basePresetId);
  }

  async save() {
    if (this.activeSavedId) {
      await this.#persist(this.activeSavedId, this.profile.name);
    } else {
      await this.saveAs(this.profile.name);
    }
  }

  async saveAs(name: string) {
    const id = crypto.randomUUID();
    this.profile.profileId = id;
    this.profile.name = name;
    await this.#persist(id, name);
  }

  async rename(name: string) {
    this.profile.name = name;
    if (this.activeSavedId) await this.#persist(this.activeSavedId, name);
    else this.dirty = true;
  }

  async duplicate() {
    await this.saveAs(`${this.profile.name} (copy)`);
  }

  async deleteSaved() {
    if (!this.activeSavedId) return;
    await db.deleteProfile(this.activeSavedId);
    this.saved = await db.listProfiles();
    this.activeSavedId = null;
    this.dirty = true;
    void db.setSetting(LAST_PROFILE_KEY, null);
  }

  async #persist(id: string, name: string) {
    const json = JSON.stringify($state.snapshot(this.profile), null, 2);
    await db.putProfile({ id, name, json, updatedAt: Date.now() });
    this.saved = await db.listProfiles();
    this.activeSavedId = id;
    this.basePresetId = null;
    this.dirty = false;
    await db.setSetting(LAST_PROFILE_KEY, id);
  }

  exportJson() {
    download(`${this.profile.name}.lvp.json`, JSON.stringify($state.snapshot(this.profile), null, 2), "application/json");
  }

  /**
   * Import profile JSON. The worker validates and migrates it (newer schemas are rejected).
   * Returns structural diagnostics; the imported profile becomes the unsaved working copy.
   */
  async importJson(text: string): Promise<Diagnostic[]> {
    const parsed = JSON.parse(text) as Profile;
    const diags = await this.worker.call({ kind: "validateProfile", profile: parsed });
    if (diags.some((d) => d.severity === "error")) return diags;
    await this.worker.call({ kind: "setProfile", profile: parsed });
    this.profile = await this.worker.call({ kind: "getProfile" }); // migrated form
    this.activeSavedId = null;
    this.basePresetId = null;
    this.dirty = true;
    void this.evaluate();
    return diags;
  }

  savedConflict(text: string): SavedProfile | undefined {
    try {
      const parsed = JSON.parse(text) as Profile;
      return this.saved.find((s) => s.id === parsed.profileId || s.name === parsed.name);
    } catch {
      return undefined;
    }
  }

  // ---- export ------------------------------------------------------------------------

  async exportPdf() {
    try {
      const bytes = await this.worker.call({ kind: "exportPdf" });
      download(`${this.profile.name}.pdf`, bytes, "application/pdf");
    } catch (e) {
      this.error = e instanceof Error ? e.message : String(e);
    }
  }

  async exportPdfPerGroup() {
    try {
      const files = await this.worker.call({ kind: "exportPdfPerGroup" });
      for (const f of files) download(`${this.profile.name} - ${f.name}.pdf`, f.bytes, "application/pdf");
    } catch (e) {
      this.error = e instanceof Error ? e.message : String(e);
    }
  }

  // ---- bindings: the profile's source slots, onto which imported layers are mapped ----

  /** Add a slot, optionally with one starting rule. */
  addBinding(name: string, rule: MatchRule | null): string {
    const id = shortId("src");
    this.update((p) => {
      p.sourceBindings.push({ id, name, required: false, rules: rule ? [rule] : [] });
    });
    return id;
  }

  /**
   * The most portable rule that matches an imported layer: its declared function when the
   * format has one (Gerber X2, IPC-2581), the file name for one-layer-per-file formats, the
   * layer name otherwise, and an explicit file+layer rule when the same layer name exists
   * in several files.
   */
  portableRule(file: string, layer: string): MatchRule {
    const src = this.sources.find((s) => s.name === file);
    const sl = src?.layers.find((l) => l.name === layer);
    if (sl?.function) return { kind: "function", pattern: sl.function };
    if (src && src.format === "gerber") return { kind: "fileName", format: "gerber", pattern: file };
    const filesWithName = this.sources.filter((s) => s.layers.some((l) => l.name === layer)).length;
    if (filesWithName > 1) return { kind: "fileLayer", file, name: layer };
    return { kind: "layerName", format: src?.format ?? "dxf", name: layer };
  }

  /** Create a slot named after an imported layer and matching it with a portable rule. */
  newBindingFromLayer(file: string, layer: string) {
    const src = this.sources.find((s) => s.name === file);
    const name = src?.format === "gerber" ? file.replace(/\.[^.]+$/, "") : layer;
    const id = this.addBinding(name, this.portableRule(file, layer));
    this.selection = { kind: "binding", id };
  }

  /** Map an imported layer onto a slot, stored as a rule in the profile. */
  mapToBinding(bindingId: string, file: string, layer: string) {
    const rule = this.portableRule(file, layer);
    this.update((p) => {
      const b = p.sourceBindings.find((x) => x.id === bindingId);
      if (!b) return;
      b.rules = [rule, ...b.rules.filter((r) => JSON.stringify(r) !== JSON.stringify(rule))];
    });
  }

  /** Layers (by name) that use a slot as an input. */
  bindingUsers(id: string): string[] {
    return this.profile.workingLayers
      .filter((l) => [...l.inputs, ...l.operands].some((o) => "source" in o && o.source === id))
      .map((l) => l.name);
  }

  /** Remove a slot; refused while a working layer still uses it. */
  removeBinding(id: string): boolean {
    if (this.bindingUsers(id).length) return false;
    this.update((p) => {
      p.sourceBindings = p.sourceBindings.filter((b) => b.id !== id);
    });
    if (this.selection.kind === "binding" && this.selection.id === id) this.selection = { kind: "none" };
    return true;
  }

  /** Which slot an imported layer is currently matched to, if any. */
  bindingOf(file: string, layer: string) {
    return this.evaluation?.bindings.find((b) => b.status === "matched" && b.file === file && b.layer === layer);
  }

  // ---- helpers -----------------------------------------------------------------------

  group(id: string) {
    return this.profile.outputGroups.find((g) => g.id === id);
  }

  layer(id: string) {
    return this.profile.workingLayers.find((l) => l.id === id);
  }

  pass(id: string) {
    for (const l of this.profile.workingLayers) {
      const p = l.passes.find((p) => p.id === id);
      if (p) return { layer: l, pass: p };
    }
    return undefined;
  }

  /** Every diagnostic from sources and the evaluation, with context. */
  get allDiagnostics(): Diagnostic[] {
    const out: Diagnostic[] = [];
    for (const s of this.sources) out.push(...s.diagnostics.map((d) => ({ ...d, context: d.context ?? s.name })));
    const ev = this.evaluation;
    if (ev) {
      out.push(...ev.diagnostics);
      for (const l of ev.layers) out.push(...l.diagnostics.map((d) => ({ ...d, context: `layer ${l.name}` })));
      for (const p of ev.passes) out.push(...p.diagnostics.map((d) => ({ ...d, context: `pass ${p.name}` })));
    }
    const rank = { error: 0, warning: 1, info: 2 };
    return out.sort((a, b) => rank[a.severity] - rank[b.severity]);
  }

  toggle(set: Set<string>, key: string) {
    const next = new Set(set);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    return next;
  }

  // ---- preview visibility (UI state only, never part of the profile) -------------------

  /** Keys of every source layer with content, as the hidden set stores them. */
  #sourceLayerKeys(): string[] {
    return this.sources.flatMap((s) =>
      s.layers.filter((l) => l.closedPaths + l.openPaths + l.hatches > 0).map((l) => `${s.index}:${l.name}`),
    );
  }

  #passIds(): string[] {
    return this.profile.workingLayers.flatMap((l) => l.passes.map((p) => p.id));
  }

  get hiddenCount(): number {
    return this.hiddenSourceLayers.size + this.hiddenLayers.size + this.hiddenPasses.size;
  }

  showAll() {
    this.hiddenSourceLayers = new Set();
    this.hiddenLayers = new Set();
    this.hiddenPasses = new Set();
  }

  toggleSourceLayer(key: string) {
    this.hiddenSourceLayers = this.toggle(this.hiddenSourceLayers, key);
  }

  /** True when every layer of the file is hidden (or the file has no content). */
  sourceFileHidden(index: number): boolean {
    const keys = this.#sourceLayerKeys().filter((k) => k.startsWith(`${index}:`));
    return keys.every((k) => this.hiddenSourceLayers.has(k));
  }

  /** Hide every layer of one file, or show them all if all were hidden. */
  toggleSourceFile(index: number) {
    const keys = this.#sourceLayerKeys().filter((k) => k.startsWith(`${index}:`));
    const show = this.sourceFileHidden(index);
    const next = new Set(this.hiddenSourceLayers);
    for (const k of keys) if (show) next.delete(k);
    else next.add(k);
    this.hiddenSourceLayers = next;
  }

  /** A layer's eye acts on the layer's region and all of its passes together. */
  toggleLayer(id: string) {
    const hide = !this.hiddenLayers.has(id);
    const passes = this.profile.workingLayers.find((l) => l.id === id)?.passes.map((p) => p.id) ?? [];
    const layers = new Set(this.hiddenLayers);
    const ps = new Set(this.hiddenPasses);
    if (hide) {
      layers.add(id);
      for (const p of passes) ps.add(p);
    } else {
      layers.delete(id);
      for (const p of passes) ps.delete(p);
    }
    this.hiddenLayers = layers;
    this.hiddenPasses = ps;
  }

  togglePass(id: string) {
    this.hiddenPasses = this.toggle(this.hiddenPasses, id);
  }

  /**
   * Show only this item: every other source layer, region and pass is hidden. A layer keeps
   * its own passes, a file keeps all its layers. Soloing the item that is already the only
   * visible one shows everything again.
   */
  solo(target: VisTarget) {
    const ownPasses = new Set(
      target.kind === "layer" ? (this.profile.workingLayers.find((l) => l.id === target.key)?.passes.map((p) => p.id) ?? []) : [],
    );
    const keep = (kind: VisTarget["kind"], key: string) =>
      target.kind === kind ? target.key === key : target.kind === "file" && kind === "source" && key.startsWith(`${target.key}:`);
    const src = new Set(this.#sourceLayerKeys().filter((k) => !keep("source", k)));
    const lay = new Set(this.profile.workingLayers.map((l) => l.id).filter((k) => !keep("layer", k)));
    const pas = new Set(this.#passIds().filter((k) => !keep("pass", k) && !ownPasses.has(k)));
    const same = (a: Set<string>, b: Set<string>) => a.size === b.size && [...a].every((k) => b.has(k));
    if (same(src, this.hiddenSourceLayers) && same(lay, this.hiddenLayers) && same(pas, this.hiddenPasses)) {
      this.showAll();
      return;
    }
    this.hiddenSourceLayers = src;
    this.hiddenLayers = lay;
    this.hiddenPasses = pas;
  }
}

/** Something the preview can show or hide. `file` keys are the source index as a string. */
export type VisTarget = { kind: "source" | "file" | "layer" | "pass"; key: string };

export function rgbCss(c: [number, number, number]): string {
  return `rgb(${c[0]}, ${c[1]}, ${c[2]})`;
}

export function rgbHex(c: [number, number, number]): string {
  return "#" + c.map((v) => v.toString(16).padStart(2, "0")).join("");
}

export function hexRgb(hex: string): [number, number, number] {
  const n = parseInt(hex.replace("#", ""), 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}
