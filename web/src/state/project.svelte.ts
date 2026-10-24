// The UI's view of the project. The worker owns the geometry; this store owns UI state and
// an editable mirror of the profile, pushes whole-profile updates, and keeps only the newest
// evaluation (older results can never overwrite a newer one).

import type {
  EvaluationSummary,
  Diagnostic,
  Preview,
  Profile,
  SourceSummary,
} from "../core/types.js";
import { GeometryWorker, WorkerCrashed } from "../worker/client.js";
import { download } from "../util/download.js";
import * as db from "./profileStore.js";
import type { SavedProfile } from "./profileStore.js";

export type Selection =
  | { kind: "none" }
  | { kind: "source"; file: number; layer: string }
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
      this.saved = await db.listProfiles();
      const lastId = await db.getSetting<string>(LAST_PROFILE_KEY);
      const last = this.saved.find((s) => s.id === lastId);
      if (last) {
        this.profile = JSON.parse(last.json) as Profile;
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
        const summary = await this.worker.call({ kind: "addDxf", name: f.name, bytes });
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
        this.sources = [...this.sources, await this.worker.call({ kind: "addDxf", name: f.name, bytes })];
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
    this.profile = structuredClone(p);
    this.activeSavedId = null;
    this.basePresetId = presetId;
    this.dirty = false;
    void db.setSetting(LAST_PROFILE_KEY, null);
    void this.evaluate();
  }

  applySaved(id: string) {
    const s = this.saved.find((x) => x.id === id);
    if (!s) return;
    this.profile = JSON.parse(s.json) as Profile;
    this.activeSavedId = id;
    this.basePresetId = null;
    this.dirty = false;
    void db.setSetting(LAST_PROFILE_KEY, id);
    void this.evaluate();
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
}

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
