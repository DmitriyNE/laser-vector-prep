// The UI's view of the project. The worker owns the geometry; this store owns UI state and
// an editable mirror of the profile, pushes whole-profile updates, and keeps only the newest
// evaluation (older results can never overwrite a newer one).

import type {
  EvaluationSummary,
  Diagnostic,
  MatchRule,
  Operand,
  Preview,
  Profile,
  SourceSummary,
  Heatmap,
} from "../core/types.js";
import { Cancelled, GeometryWorker, WorkerCrashed, type Progress } from "../worker/client.js";
import { download, shortId } from "../util/download.js";
import type { Edit, Removal } from "./graph.js";
import * as db from "./profileStore.js";
import type { SavedProfile } from "./profileStore.js";

export type Selection =
  | { kind: "none" }
  | { kind: "source"; file: number; layer: string }
  | { kind: "binding"; id: string }
  | { kind: "layer"; id: string }
  | { kind: "pass"; id: string }
  | { kind: "group"; id: string };

const LAST_PROFILE_KEY = "lastProfile";

export class ProjectStore {
  worker = new GeometryWorker();

  ready = $state(false);
  coreVersion = $state("…");
  threads: number | null = $state(null);
  error: string | null = $state(null);
  /** Neutral status, such as a cancelled computation. */
  notice: string | null = $state(null);

  sources: SourceSummary[] = $state([]);
  presets: Profile[] = $state([]);
  /** File extensions the core can read (lower case, no dot). */
  acceptedExtensions: string[] = $state(["dxf"]);
  saved: SavedProfile[] = $state([]);
  profile: Profile = $state({
    schemaVersion: 3,
    profileId: "empty",
    name: "Empty",
    sourceBindings: [],
    workingLayers: [],
    outputGroups: [],
    export: { strokeWidthMm: 0.0254, marginMm: 5, separateFiles: false, flattenToleranceMm: 0.001 },
    optimizer: { mode: "minimumJump", footprintMm: 0.1, bleedWeight: 1, protected: [], cellMm: 0.05, maxSweeps: 20 },
  });
  /** Saved profile the edits are based on; `null` while based on a preset or an import. */
  activeSavedId: string | null = $state(null);
  /** Preset the current profile derives from (for Reset) when not based on a saved one. */
  basePresetId: string | null = $state(null);
  dirty = $state(false);
  canUndo = $state(false);
  canRedo = $state(false);

  evaluation: EvaluationSummary | null = $state(null);
  evaluating = $state(false);
  importing = $state(false);
  /** Bumps whenever preview geometry may have changed. */
  previewRevision = $state(0);

  selection: Selection = $state({ kind: "none" });
  hiddenSourceLayers = $state(new Set<string>());
  hiddenLayers = $state(new Set<string>());
  hiddenPasses = $state(new Set<string>());
  showJumps = $state(false);
  showHeatmap = $state(false);
  showAudit = $state(false);
  showSources = $state(true);
  showLayers = $state(true);

  /** The evaluation loop while it runs; callers awaiting `evaluate()` share it. */
  #evalLoop: Promise<void> | null = null;
  /** Another evaluation is wanted after the one in flight (which is being cancelled). */
  #evalAgain = false;
  /** Epochs of the evaluation and import in flight (0: none), for `cancel()`. */
  #evalEpoch = 0;
  #importEpoch = 0;
  #debounce: ReturnType<typeof setTimeout> | undefined;
  /** Imported files, kept (in memory only) so sources can be re-imported after a crash. */
  #files: File[] = [];
  /** The profile after each edit (JSON), for undo and redo; `#at` is the current entry. */
  #history: string[] = [];
  #at = 0;
  /** The entry that matches the saved profile or preset (-1: none does). */
  #cleanAt = 0;
  #lastEdit = 0;
  /** The field the last edit was typed into, if any. */
  #lastField: Element | null = null;

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
      this.#resetHistory();
      this.ready = true;
      await this.evaluate();
    } catch (e) {
      this.error = e instanceof Error ? e.message : String(e);
    }
  }

  // ---- sources -----------------------------------------------------------------------

  async addFiles(files: Iterable<File>) {
    // Snapshot first: a FileList from an <input> is live and empties when the input is
    // reset, which happened between the awaits below and dropped every file but the first.
    this.importing = true;
    this.notice = null;
    try {
      for (const f of Array.from(files)) {
        const epoch = this.worker.nextEpoch();
        this.#importEpoch = epoch;
        try {
          const bytes = new Uint8Array(await f.arrayBuffer());
          const summary = await this.worker.call({ kind: "addSource", name: f.name, bytes, epoch });
          this.sources = [...this.sources, summary];
          this.#files.push(f);
        } catch (e) {
          if (e instanceof Cancelled) {
            this.notice = `Import of ${f.name} cancelled.`;
            break;
          }
          this.error = `${f.name}: ${e instanceof Error ? e.message : String(e)}`;
        }
      }
    } finally {
      this.#importEpoch = 0;
      this.importing = false;
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
        const epoch = this.worker.nextEpoch();
        this.sources = [...this.sources, await this.worker.call({ kind: "addSource", name: f.name, bytes, epoch })];
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
    this.#record();
    this.scheduleEvaluate();
  }

  // ---- undo and redo of profile edits -------------------------------------------------

  /** Start the history at the profile just loaded. */
  #resetHistory(clean = true) {
    this.#history = [JSON.stringify($state.snapshot(this.profile))];
    this.#at = 0;
    this.#cleanAt = clean ? 0 : -1;
    this.#lastEdit = 0;
    this.#lastField = null;
    this.#syncHistory();
  }

  /** Record the profile after an edit. Typing on in the same field (each keystroke is an
   * edit) extends the step; anything else is a step of its own. */
  #record() {
    const snapshot = JSON.stringify($state.snapshot(this.profile));
    if (snapshot === this.#history[this.#at]) return;
    this.#history.length = this.#at + 1;
    if (this.#cleanAt > this.#at) this.#cleanAt = -1;
    const now = Date.now();
    const el = globalThis.document?.activeElement ?? null;
    const field =
      (el instanceof HTMLInputElement && !["checkbox", "radio", "button", "submit", "file"].includes(el.type)) || el instanceof HTMLTextAreaElement ? el : null;
    if (this.#at > 0 && field !== null && field === this.#lastField && now - this.#lastEdit < 1000) {
      if (this.#cleanAt === this.#at) this.#cleanAt = -1;
      this.#history[this.#at] = snapshot;
    } else {
      this.#history.push(snapshot);
      this.#at++;
    }
    this.#lastEdit = now;
    this.#lastField = field;
    // A long session keeps the last 200 steps.
    if (this.#history.length > 200) {
      this.#history.shift();
      this.#at--;
      this.#cleanAt--;
    }
    this.#syncHistory();
  }

  undo() {
    if (this.#at === 0) return;
    this.#at--;
    this.#restore();
  }

  redo() {
    if (this.#at >= this.#history.length - 1) return;
    this.#at++;
    this.#restore();
  }

  #restore() {
    this.profile = JSON.parse(this.#history[this.#at]!) as Profile;
    this.dirty = this.#at !== this.#cleanAt;
    this.#lastField = null;
    this.#syncHistory();
    // What was selected may be gone in the restored profile.
    const s = this.selection;
    const exists =
      s.kind === "layer" ? !!this.layer(s.id) : s.kind === "pass" ? !!this.pass(s.id) : s.kind === "group" ? !!this.group(s.id) : s.kind === "binding" ? this.profile.sourceBindings.some((b) => b.id === s.id) : true;
    if (!exists) this.selection = { kind: "none" };
    this.scheduleEvaluate();
  }

  #syncHistory() {
    this.canUndo = this.#at > 0;
    this.canRedo = this.#at < this.#history.length - 1;
  }

  scheduleEvaluate() {
    clearTimeout(this.#debounce);
    this.#debounce = setTimeout(() => void this.evaluate(), 150);
  }

  /**
   * Evaluate the current profile. At most one evaluation is in flight: a call while one runs
   * cancels it and evaluates again once it has stopped, so an edit never waits for a stale
   * result and a superseded result is never shown. Resolves when the newest profile has
   * been evaluated (or the run was cancelled).
   */
  evaluate(): Promise<void> {
    if (this.#evalLoop) {
      this.#evalAgain = true;
      if (this.#evalEpoch) this.worker.cancel(this.#evalEpoch);
      return this.#evalLoop;
    }
    this.#evalLoop = this.#evaluations().finally(() => {
      this.#evalLoop = null;
    });
    return this.#evalLoop;
  }

  async #evaluations() {
    this.evaluating = true;
    try {
      do {
        this.#evalAgain = false;
        const epoch = this.worker.nextEpoch();
        this.#evalEpoch = epoch;
        try {
          await this.worker.call({ kind: "setProfile", profile: $state.snapshot(this.profile) });
          const ev = await this.worker.call({ kind: "evaluate", epoch });
          if (this.#evalAgain) continue; // superseded while it ran
          this.evaluation = ev;
          this.previewRevision++;
          this.error = null;
          this.notice = null;
        } catch (e) {
          if (e instanceof Cancelled) {
            if (!this.#evalAgain) this.notice = "Evaluation cancelled; the preview shows the previous result.";
          } else if (!this.#evalAgain && !(e instanceof WorkerCrashed)) {
            this.error = e instanceof Error ? e.message : String(e);
          }
        } finally {
          this.#evalEpoch = 0;
        }
      } while (this.#evalAgain);
    } finally {
      this.evaluating = false;
    }
  }

  /** Stop the running import or evaluation; the previous result stays. */
  cancel() {
    this.#evalAgain = false;
    clearTimeout(this.#debounce);
    if (this.#evalEpoch) this.worker.cancel(this.#evalEpoch);
    if (this.#importEpoch) this.worker.cancel(this.#importEpoch);
  }

  /** Progress of the worker's running computation (polled by the UI while busy). */
  progress(): Progress | null {
    return this.worker.progress();
  }

  preview(kind: "pass" | "layer", id: string): Promise<Preview | null> {
    return kind === "pass"
      ? this.worker.call({ kind: "previewPass", id })
      : this.worker.call({ kind: "previewLayer", id });
  }

  previewSource(file: number, layer: string): Promise<Preview | null> {
    return this.worker.call({ kind: "previewSource", file, layer });
  }

  /** Hypothetical jumps of an output group's planned order. */
  previewJumps(groupId: string): Promise<Preview | null> {
    return this.worker.call({ kind: "previewJumps", id: groupId });
  }

  /** Missed area (or, with `spill`, swept area outside the region) of a pass's coverage audit. */
  previewAudit(passId: string, spill: boolean): Promise<Preview | null> {
    return this.worker.call({ kind: "previewAudit", id: passId, spill });
  }

  /** Footprint coverage raster of a group over the protected regions, if any. */
  heatmap(groupId: string): Promise<Heatmap | null> {
    return this.worker.call({ kind: "heatmap", id: groupId });
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
    this.#resetHistory();
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
    this.#resetHistory();
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
    // Saving may rename the profile: the current step is the saved profile from now on.
    this.#history[this.#at] = JSON.stringify($state.snapshot(this.profile));
    this.#cleanAt = this.#at;
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
    this.#resetHistory(false);
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

  async exportSvg() {
    try {
      const bytes = await this.worker.call({ kind: "exportSvg" });
      download(`${this.profile.name}.svg`, bytes, "image/svg+xml");
    } catch (e) {
      this.error = e instanceof Error ? e.message : String(e);
    }
  }

  async exportDxf() {
    try {
      const bytes = await this.worker.call({ kind: "exportDxf" });
      download(`${this.profile.name}.dxf`, bytes, "application/dxf");
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

  // ---- structure: layers, passes, groups, and the edits drawn on the graph -------------

  /** Add a working layer (a region, or an envelope: a bounded domain) and select it. */
  addLayer(kind: "region" | "envelope" = "region"): string {
    const id = shortId("wl");
    this.update((p) => {
      p.workingLayers.push({
        id,
        name: kind === "envelope" ? `Envelope ${p.workingLayers.length + 1}` : `Layer ${p.workingLayers.length + 1}`,
        kind,
        inputs: [],
        operands: [],
        op: "union",
        envelope: kind === "envelope" ? { provider: "boundingBox", marginMm: 0.5 } : null,
        interpretation: { loopRule: "mergeThenNest", strokeWidth: "original" },
        passes: [],
        enabled: true,
        // Envelopes are domains for other layers; they never mark unless asked explicitly.
        referenceOnly: kind === "envelope",
      });
    });
    this.selection = { kind: "layer", id };
    return id;
  }

  /** Add a pass to a layer (marking in the first output group) and select it. */
  addPass(layerId: string): string | null {
    const layer = this.layer(layerId);
    if (!layer) return null;
    const id = shortId("pass");
    const group = this.profile.outputGroups[0]?.id ?? "";
    this.update(() => {
      layer.passes.push({
        id,
        name: layer.kind === "region" ? "Contour fill" : "Passthrough",
        pattern: layer.kind === "region" ? { kind: "contourFill", pitchMm: 0.02, firstOffsetMm: null, gapFill: true } : { kind: "passthrough", reverse: false },
        group,
        enabled: true,
      });
    });
    this.selection = { kind: "pass", id };
    return id;
  }

  /** Delete a working layer with its passes; layers reading it and the optimizer's
   * protected list forget it. */
  deleteLayer(id: string) {
    const other = (o: Operand) => !("layer" in o && o.layer === id);
    this.update((p) => {
      p.workingLayers = p.workingLayers.filter((l) => l.id !== id);
      for (const l of p.workingLayers) {
        l.inputs = l.inputs.filter(other);
        l.operands = l.operands.filter(other);
      }
      p.optimizer.protected = p.optimizer.protected.filter((x) => x !== id);
    });
    if ((this.selection.kind === "layer" && this.selection.id === id) || (this.selection.kind === "pass" && !this.pass(this.selection.id))) {
      this.selection = { kind: "none" };
    }
  }

  deletePass(id: string) {
    this.update((p) => {
      for (const l of p.workingLayers) l.passes = l.passes.filter((x) => x.id !== id);
    });
    if (this.selection.kind === "pass" && this.selection.id === id) this.selection = { kind: "none" };
  }

  addGroup(): string {
    const id = shortId("grp");
    this.update((p) => {
      p.outputGroups.push({ id, name: `Group ${p.outputGroups.length + 1}`, color: [0, 160, 0], strokeWidthMm: null, export: true, visible: true, orderMode: null });
    });
    this.selection = { kind: "group", id };
    return id;
  }

  /** Passes (by name) that mark in a group. */
  groupUsers(id: string): string[] {
    return this.profile.workingLayers.flatMap((l) => l.passes).filter((p) => p.group === id).map((p) => p.name);
  }

  /** Remove an output group; refused while a pass marks in it. */
  removeGroup(id: string): boolean {
    if (this.groupUsers(id).length) return false;
    this.update((p) => {
      p.outputGroups = p.outputGroups.filter((g) => g.id !== id);
    });
    if (this.selection.kind === "group" && this.selection.id === id) this.selection = { kind: "none" };
    return true;
  }

  /** Apply an edit drawn on the graph (validated by `connectionEdit`). */
  applyEdit(e: Edit) {
    switch (e.kind) {
      case "map":
        this.mapToBinding(e.slot, e.file, e.layer);
        break;
      case "operand":
        this.update((p) => {
          const l = p.workingLayers.find((x) => x.id === e.layer);
          if (l) (e.group === "a" ? l.inputs : l.operands).push(e.from);
        });
        break;
      case "movePass":
        this.update((p) => {
          const target = p.workingLayers.find((x) => x.id === e.layer);
          const owner = p.workingLayers.find((x) => x.passes.some((ps) => ps.id === e.pass));
          if (!target || !owner) return;
          const pass = owner.passes.find((ps) => ps.id === e.pass)!;
          owner.passes = owner.passes.filter((ps) => ps.id !== e.pass);
          target.passes.push(pass);
        });
        break;
      case "passGroup":
        this.update((p) => {
          for (const l of p.workingLayers) for (const ps of l.passes) if (ps.id === e.pass) ps.group = e.group;
        });
        break;
    }
  }

  /** Apply a deletion made on the graph (see `nodeRemoval` and `edgeRemoval`). */
  applyRemoval(r: Removal) {
    switch (r.kind) {
      case "operand": {
        const keep = (o: Operand) => !(("source" in o && "source" in r.from && o.source === r.from.source) || ("layer" in o && "layer" in r.from && o.layer === r.from.layer));
        this.update((p) => {
          const l = p.workingLayers.find((x) => x.id === r.layer);
          if (!l) return;
          if (r.group === "a") l.inputs = l.inputs.filter(keep);
          else l.operands = l.operands.filter(keep);
        });
        break;
      }
      case "unmap":
        this.update((p) => {
          const b = p.sourceBindings.find((x) => x.id === r.slot);
          if (b) b.rules = b.rules.filter((_, i) => !r.rules.includes(i));
        });
        break;
      case "layer":
        this.deleteLayer(r.id);
        break;
      case "pass":
        this.deletePass(r.id);
        break;
      case "slot":
        this.removeBinding(r.id);
        break;
      case "group":
        this.removeGroup(r.id);
        break;
      case "file":
        void this.removeSource(r.index);
        break;
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
