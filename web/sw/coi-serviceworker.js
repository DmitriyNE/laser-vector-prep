/*! coi-serviceworker v0.1.7 - Guido Zuidhof and contributors, licensed under MIT.
 *  https://github.com/gzuidhof/coi-serviceworker
 *
 *  Modified for Laser Vector Prep: the worker half also precaches the static bundle so the
 *  app keeps working offline after the first load. The build injects the precache list and
 *  cache name (see web/vite.config.ts). Because precaching delays activation, the page half
 *  reloads only once the registration is ready (active) and at most once per session,
 *  instead of on `updatefound`. */
let coepCredentialless = false;
if (typeof window === 'undefined') {
    const CACHE_NAME = "__CACHE_NAME__";
    const PRECACHE = __PRECACHE__;

    self.addEventListener("install", (event) => {
        event.waitUntil((async () => {
            const cache = await caches.open(CACHE_NAME);
            await cache.addAll(PRECACHE.map((p) => new URL(p, self.location.href).href));
            await self.skipWaiting();
        })());
    });
    self.addEventListener("activate", (event) => {
        event.waitUntil((async () => {
            for (const key of await caches.keys()) {
                if (key !== CACHE_NAME) await caches.delete(key);
            }
            await self.clients.claim();
        })());
    });

    self.addEventListener("message", (ev) => {
        if (!ev.data) {
            return;
        } else if (ev.data.type === "deregister") {
            self.registration
                .unregister()
                .then(() => {
                    return self.clients.matchAll();
                })
                .then(clients => {
                    clients.forEach((client) => client.navigate(client.url));
                });
        } else if (ev.data.type === "coepCredentialless") {
            coepCredentialless = ev.data.value;
        }
    });

    self.addEventListener("fetch", function (event) {
        const r = event.request;
        if (r.cache === "only-if-cached" && r.mode !== "same-origin") {
            return;
        }

        const request = (coepCredentialless && r.mode === "no-cors")
            ? new Request(r, {
                credentials: "omit",
            })
            : r;
        event.respondWith((async () => {
            let response;
            const sameOrigin = new URL(r.url).origin === self.location.origin;
            if (sameOrigin && r.method === "GET") {
                // Cache first: the bundle is content-addressed, so a hit is always current.
                response = await caches.match(r);
                if (!response) {
                    try {
                        response = await fetch(request);
                        if (response.ok && response.type === "basic") {
                            const cache = await caches.open(CACHE_NAME);
                            cache.put(r, response.clone());
                        }
                    } catch (e) {
                        if (r.mode === "navigate") {
                            response = await caches.match(new URL("./index.html", self.location.href).href);
                        }
                        if (!response) throw e;
                    }
                }
            } else {
                response = await fetch(request);
            }
            if (response.status === 0) {
                return response;
            }

            const newHeaders = new Headers(response.headers);
            newHeaders.set("Cross-Origin-Embedder-Policy",
                coepCredentialless ? "credentialless" : "require-corp"
            );
            if (!coepCredentialless) {
                newHeaders.set("Cross-Origin-Resource-Policy", "cross-origin");
            }
            newHeaders.set("Cross-Origin-Opener-Policy", "same-origin");

            return new Response(response.body, {
                status: response.status,
                statusText: response.statusText,
                headers: newHeaders,
            });
        })());
    });

} else {
    (() => {
        // You can customize the behavior of this script through a global `coi` variable.
        const coi = {
            shouldRegister: () => true,
            shouldDeregister: () => false,
            coepCredentialless: () => !(window.chrome || window.netscape),
            doReload: () => window.location.reload(),
            quiet: false,
            ...window.coi
        };

        const n = navigator;

        if (n.serviceWorker && n.serviceWorker.controller) {
            n.serviceWorker.controller.postMessage({
                type: "coepCredentialless",
                value: coi.coepCredentialless(),
            });

            if (coi.shouldDeregister()) {
                n.serviceWorker.controller.postMessage({ type: "deregister" });
            }
        }

        // If we're already coi: do nothing. Perhaps it's due to this script doing its job, or COOP/COEP are
        // already set from the origin server. Also if the browser has no notion of crossOriginIsolated, just give up here.
        if (window.crossOriginIsolated !== false || !coi.shouldRegister()) return;

        if (!window.isSecureContext) {
            !coi.quiet && console.log("COOP/COEP Service Worker not registered, a secure context is required.");
            return;
        }

        // In some environments (e.g. Chrome incognito mode) this won't be available
        if (n.serviceWorker) {
            const RELOADED_KEY = "coi-reloaded";
            let alreadyReloaded = false;
            try {
                alreadyReloaded = window.sessionStorage.getItem(RELOADED_KEY) === "1";
            } catch (e) {
                // sessionStorage can be unavailable; then we simply allow one reload.
            }
            n.serviceWorker.register(window.document.currentScript.src).then(
                async (registration) => {
                    !coi.quiet && console.log("COOP/COEP Service Worker registered", registration.scope);
                    // Wait for an *active* worker (install may precache for a while), then reload
                    // once so the navigation is served with the isolation headers.
                    await n.serviceWorker.ready;
                    if (alreadyReloaded) {
                        !coi.quiet && console.log("COOP/COEP Service Worker active, but the page is still not isolated after a reload; giving up.");
                        return;
                    }
                    try {
                        window.sessionStorage.setItem(RELOADED_KEY, "1");
                    } catch (e) {
                        // see above
                    }
                    !coi.quiet && console.log("Reloading page to make use of COOP/COEP Service Worker.");
                    coi.doReload();
                },
                (err) => {
                    !coi.quiet && console.error("COOP/COEP Service Worker failed to register:", err);
                }
            );
        }
    })();
}
