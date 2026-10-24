// Local, offline profile persistence (IndexedDB). Profiles only: never drawing bytes.

export interface SavedProfile {
  id: string;
  name: string;
  json: string;
  updatedAt: number;
}

const DB_NAME = "lvp";
const PROFILES = "profiles";
const SETTINGS = "settings";

function request<T>(r: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    r.onsuccess = () => resolve(r.result);
    r.onerror = () => reject(r.error ?? new Error("IndexedDB request failed"));
  });
}

let dbPromise: Promise<IDBDatabase> | null = null;

function open(): Promise<IDBDatabase> {
  if (!dbPromise) {
    dbPromise = new Promise((resolve, reject) => {
      const req = indexedDB.open(DB_NAME, 1);
      req.onupgradeneeded = () => {
        const db = req.result;
        if (!db.objectStoreNames.contains(PROFILES)) db.createObjectStore(PROFILES, { keyPath: "id" });
        if (!db.objectStoreNames.contains(SETTINGS)) db.createObjectStore(SETTINGS);
      };
      req.onsuccess = () => resolve(req.result);
      req.onerror = () => reject(req.error ?? new Error("IndexedDB open failed"));
    });
  }
  return dbPromise;
}

export async function listProfiles(): Promise<SavedProfile[]> {
  const db = await open();
  const all = await request(db.transaction(PROFILES).objectStore(PROFILES).getAll());
  return (all as SavedProfile[]).sort((a, b) => a.name.localeCompare(b.name));
}

export async function putProfile(p: SavedProfile): Promise<void> {
  const db = await open();
  await request(db.transaction(PROFILES, "readwrite").objectStore(PROFILES).put(p));
}

export async function deleteProfile(id: string): Promise<void> {
  const db = await open();
  await request(db.transaction(PROFILES, "readwrite").objectStore(PROFILES).delete(id));
}

export async function getSetting<T>(key: string): Promise<T | undefined> {
  const db = await open();
  return (await request(db.transaction(SETTINGS).objectStore(SETTINGS).get(key))) as T | undefined;
}

export async function setSetting(key: string, value: unknown): Promise<void> {
  const db = await open();
  await request(db.transaction(SETTINGS, "readwrite").objectStore(SETTINGS).put(value, key));
}
