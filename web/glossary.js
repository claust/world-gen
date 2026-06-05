// FloraForge glossary popups.
//
// Mark any inline term with `<button class="term" data-term="KEY">…</button>`
// (KEY must match an entry below). This script styles those terms, builds a
// single dialog, and opens it with the matching description on click.
//
// The popup is dismissible by clicking the backdrop or pressing Escape.
(() => {
    "use strict";

    // Each entry: a short title and an array of paragraphs (up to five).
    const TERMS = {
        "game-loop": {
            title: "The game loop",
            body: [
                "A real-time engine is, at its heart, one loop that never stops running. Every lap — every “tick” — it reads input, advances the simulation by a tiny slice of time, draws the result, and shows it on screen. Then it does the whole thing again, ideally 60 or more times a second.",
                "What makes it a loop rather than a script is that nothing is ever “finished.” The world is redrawn from scratch each frame, so motion is just the difference between one frame and the next. A character walking across the screen isn’t animated by a timer somewhere; it’s simply drawn a few pixels further along on every pass through the loop.",
                "FloraForge runs its loop in “poll” mode: the moment a frame finishes, it asks the window system for the next one immediately, rather than waiting for a fixed clock. That keeps the frame rate as high as the machine allows, and the elapsed time between frames (often written “dt”) is measured each lap so the simulation runs at the same speed whether you’re drawing at 30 or 300 frames per second.",
            ],
        },
        "update-render": {
            title: "Update vs. render",
            body: [
                "Every frame splits cleanly into two halves. The update step is the “think” phase: it moves the camera, advances the clock, streams new terrain, grows plants — all pure logic that changes the state of the world. The render step is the “draw” phase: it takes that state and turns it into pixels.",
                "Keeping them separate is a deliberate discipline. Update only cares about how much time has passed, never about how the world looks; render only reads the world, never changes it. Because of that split, the simulation behaves identically no matter how fast or slow the graphics are — a slow machine just draws the same world less often.",
                "It also makes the engine easier to reason about. If something moves wrong, the bug is in update. If something looks wrong, the bug is in render. The two halves almost never tangle.",
            ],
        },
        "procedural-generation": {
            title: "Procedural generation",
            body: [
                "Procedural generation means building content from rules and math instead of placing it by hand. Rather than an artist sculpting every hill, a function decides the height of the ground at each point in the world, and the landscape falls out of that function.",
                "The usual ingredient is noise — a smooth, random-looking field of values you can sample anywhere. Layer a few scales of noise together (broad rolling shapes plus fine bumpy detail) and you get terrain that feels natural but was never drawn. Feed the same idea moisture and temperature and you can decide where forests, deserts, and grasslands belong.",
                "The big payoff is that the world is effectively infinite and consistent. Because everything is derived from a starting number (the “seed”), the same seed always rebuilds the same world, and you only ever have to store the seed — not the millions of vertices it expands into.",
            ],
        },
        "chunk-streaming": {
            title: "Chunk streaming",
            body: [
                "A world too big to hold in memory at once is sliced into a grid of fixed-size tiles called chunks — in FloraForge, 256-metre squares. Only the chunks near the camera are ever loaded; the rest don’t exist yet, as far as the engine is concerned.",
                "As you fly, the engine continuously loads chunks entering a radius around you and discards chunks that fall out the back. This rolling window is what lets you travel forever without the memory or the workload ever growing — the count of live chunks stays roughly constant no matter how far you go.",
                "The tricky part is doing it without stutter. Generating a chunk’s terrain takes real work, so FloraForge hands that work to background threads and only collects the finished result later. The main loop never blocks waiting for a chunk; it just draws what’s ready and picks up new chunks as they arrive.",
            ],
        },
        "compute-shader": {
            title: "Compute shader",
            body: [
                "A shader is a small program that runs on the graphics card instead of the main processor. The famous ones draw triangles, but a compute shader is the general-purpose kind: it does raw parallel math, with no drawing implied. You hand the GPU a problem split into thousands of tiny identical tasks and it chews through them all at once.",
                "FloraForge uses one to build terrain. For each of a chunk’s 129×129 grid points, the compute shader reads a height and moisture value and writes out a finished mesh vertex — position, normal, colour. Because the GPU runs thousands of those grid points simultaneously, an entire chunk’s surface is assembled in a fraction of the time a CPU loop would take.",
                "The win isn’t only speed; it’s location. The vertex data is produced on the GPU, where it’s about to be drawn anyway, so it never has to make the slow trip across from main memory. The terrain is born where it lives.",
            ],
        },
        "instanced-rendering": {
            title: "Instanced rendering",
            body: [
                "Drawing a forest the naive way — one draw command per tree — would drown the GPU in overhead, since each command carries a fixed cost no matter how small the object. Instanced rendering is the fix: you describe the shape once, then ask the GPU to stamp out thousands of copies of it in a single command.",
                "The shape (a tree mesh, say) is uploaded just once. Alongside it goes a compact list of “instances” — one small record per copy, holding just what differs: position, rotation, scale, maybe a tint. The GPU walks that list and draws the shared shape once per entry, each time nudged into place by its instance record.",
                "FloraForge uses this for all its vegetation and houses. A whole chunk’s worth of a given species is one draw call with a per-chunk instance buffer, which is why a densely planted world stays cheap to render.",
            ],
        },
        "frustum-culling": {
            title: "Frustum culling",
            body: [
                "The view frustum is the truncated pyramid of space the camera can actually see — bounded by the screen edges, a near plane just in front of the lens, and a far plane in the distance. Anything outside that pyramid will never appear in the final image.",
                "Frustum culling is the optimisation of checking, before drawing, whether an object lies inside the frustum at all — and skipping it entirely if not. There’s no point spending GPU time on a forest behind the camera or a hill off to the side.",
                "FloraForge tests each chunk’s bounding box against the frustum every frame and only submits the ones that survive, for terrain, plants, and water alike. In a wide-open world where most chunks are off-screen at any moment, this is one of the largest single savings the renderer makes.",
                "The word “frustum” is Latin for a piece, bit, or morsel — something broken or cut off. Geometers borrowed it for the shape you get when you slice the top off a cone or pyramid with a cut parallel to its base, leaving a solid with its tip removed. A camera’s view is exactly that: a pyramid with its point at the lens, its tip lopped off by the near plane and its base capped by the far plane. (The plural, if you ever need it, is “frusta.”)",
            ],
        },
        "bind-group": {
            title: "Bind groups",
            body: [
                "Shaders running on the GPU need data to work with — the camera’s view matrix, the current time of day, lighting parameters, textures. A bind group is the bundle that makes that data available to a shader: a named collection of resources the GPU plugs in before a draw.",
                "The reason to group them is performance. Data that changes once per frame (the camera, the clock) goes in one group; data that changes per material (a particular surface’s lighting) goes in another. The engine can swap the per-material group many times while leaving the per-frame group untouched, which means less work re-binding things the GPU already has.",
                "FloraForge follows exactly that layout — group 0 holds the per-frame uniforms, group 1 holds per-material settings — so the common case of “same camera, different surface” costs as little as possible.",
            ],
        },
        "swapchain": {
            title: "The swapchain & presenting",
            body: [
                "You never draw directly to the screen. Instead the engine draws into an off-screen image, and only when that image is completely finished does it get shown all at once. The swapchain is the small rotation of those images — typically two or three — that the engine and the display hand back and forth.",
                "While the screen is showing one image, the engine is busy drawing the next into a different one. When the new frame is done it is “presented” — swapped in to become what the display reads from — and the old image is recycled for the frame after that. This double-buffering is what prevents you from ever seeing a half-drawn picture.",
                "Acquiring the next blank image to draw into can make the engine wait, because it can’t reuse an image the display is still reading. FloraForge measures the length of that wait: when it’s long, the GPU is the bottleneck, and that single number tells the engine which half of the frame to optimise.",
            ],
        },
        "webgpu": {
            title: "WebGPU & wgpu",
            body: [
                "WebGPU is the modern standard for talking to a graphics card — the successor to the older WebGL. It exposes the GPU’s real capabilities (including compute shaders) through a clean, explicit interface, and it runs both on the native desktop and inside the browser.",
                "“wgpu” is the specific implementation FloraForge builds on: a Rust library that speaks WebGPU and translates it down to whatever the machine actually has — Metal on a Mac, Vulkan on Linux, Direct3D on Windows, or WebGPU itself in the browser. The engine writes its graphics code once against wgpu and that single codebase runs everywhere.",
                "That portability is the whole point. The same Rust renderer that produces the desktop build also compiles to the browser build, with no separate graphics path to maintain.",
            ],
        },
        "webassembly": {
            title: "WebAssembly",
            body: [
                "WebAssembly (often shortened to “wasm”) is a compact, fast, low-level format that browsers can run at close to native speed. It’s the target that lets languages like Rust, C++, and Go run on a web page instead of only JavaScript.",
                "FloraForge is written entirely in Rust, then compiled to a WebAssembly bundle. When you launch the browser build, the page downloads that bundle and hands it the canvas; from there the same engine code that runs on the desktop drives the simulation and the WebGPU renderer directly in the tab.",
                "The result is a genuine 3D engine running in a browser with no plugins and no server doing the work — the streaming terrain, the camera, and the day/night cycle are all executing locally as compiled code.",
            ],
        },
    };

    const STYLE = `
        button.term {
            appearance: none;
            font: inherit;
            color: var(--green-bright, #3ecf6e);
            background: rgba(62, 207, 110, 0.08);
            border: 0;
            border-bottom: 1px dashed rgba(124, 255, 159, 0.55);
            border-radius: 4px;
            padding: 0 0.12em;
            margin: 0;
            cursor: pointer;
            transition: color 160ms ease, background 160ms ease, border-color 160ms ease;
        }
        button.term::after {
            content: "\\2009\\25C9";
            font-size: 0.62em;
            vertical-align: 0.18em;
            opacity: 0.7;
        }
        button.term:hover,
        button.term:focus-visible {
            color: var(--green-glow, #7cff9f);
            background: rgba(124, 255, 159, 0.16);
            border-bottom-color: var(--green-glow, #7cff9f);
            outline: none;
        }
        button.term:focus-visible {
            outline: 2px solid rgba(124, 255, 159, 0.5);
            outline-offset: 2px;
        }

        .glossary-overlay {
            position: fixed;
            inset: 0;
            z-index: 1000;
            display: flex;
            align-items: center;
            justify-content: center;
            padding: 1.5rem;
            background: rgba(4, 10, 6, 0.62);
            backdrop-filter: blur(6px);
            opacity: 0;
            pointer-events: none;
            transition: opacity 200ms ease;
        }
        .glossary-overlay[data-open="true"] {
            opacity: 1;
            pointer-events: auto;
        }

        .glossary-dialog {
            width: min(560px, 100%);
            max-height: min(80vh, 640px);
            overflow-y: auto;
            border-radius: 22px;
            background: var(--panel-strong, rgba(11, 24, 13, 0.92));
            border: 1px solid rgba(62, 207, 110, 0.22);
            box-shadow: 0 30px 90px rgba(0, 0, 0, 0.5);
            padding: 2rem 2rem 2.2rem;
            color: var(--cream, #f0ede4);
            transform: translateY(12px) scale(0.98);
            transition: transform 200ms ease;
            font-family: "DM Sans", sans-serif;
            line-height: 1.65;
        }
        .glossary-overlay[data-open="true"] .glossary-dialog {
            transform: translateY(0) scale(1);
        }

        .glossary-head {
            display: flex;
            align-items: flex-start;
            justify-content: space-between;
            gap: 1rem;
            margin-bottom: 1.2rem;
        }
        .glossary-eyebrow {
            font-family: "Space Grotesk", sans-serif;
            font-size: 0.72rem;
            font-weight: 500;
            letter-spacing: 0.2em;
            text-transform: uppercase;
            color: var(--green-bright, #3ecf6e);
            opacity: 0.84;
            margin-bottom: 0.5rem;
        }
        .glossary-title {
            font-family: "Space Grotesk", sans-serif;
            letter-spacing: -0.02em;
            font-size: 1.5rem;
            line-height: 1.12;
            margin: 0;
            color: var(--cream, #f0ede4);
        }
        .glossary-close {
            appearance: none;
            flex: 0 0 auto;
            width: 2.2rem;
            height: 2.2rem;
            border-radius: 999px;
            border: 1px solid rgba(240, 237, 228, 0.16);
            background: rgba(255, 255, 255, 0.04);
            color: var(--cream, #f0ede4);
            font-size: 1.2rem;
            line-height: 1;
            cursor: pointer;
            transition: background 160ms ease, transform 160ms ease;
        }
        .glossary-close:hover { background: rgba(255, 255, 255, 0.1); transform: rotate(90deg); }
        .glossary-close:focus-visible { outline: 2px solid rgba(124, 255, 159, 0.5); outline-offset: 2px; }

        .glossary-body p {
            color: var(--sand, #c7b99d);
            font-size: 0.98rem;
            margin: 0 0 1rem;
        }
        .glossary-body p:last-child { margin-bottom: 0; }

        @media (max-width: 640px) {
            .glossary-dialog { padding: 1.5rem 1.3rem 1.6rem; }
            .glossary-title { font-size: 1.3rem; }
        }
    `;

    function init() {
        const triggers = Array.from(document.querySelectorAll("button.term[data-term]"));
        if (triggers.length === 0) return;

        const style = document.createElement("style");
        style.textContent = STYLE;
        document.head.appendChild(style);

        const overlay = document.createElement("div");
        overlay.className = "glossary-overlay";
        overlay.setAttribute("data-open", "false");
        overlay.innerHTML = `
            <div class="glossary-dialog" role="dialog" aria-modal="true" aria-labelledby="glossary-title">
                <div class="glossary-head">
                    <div>
                        <div class="glossary-eyebrow">Engine concept</div>
                        <h2 class="glossary-title" id="glossary-title"></h2>
                    </div>
                    <button class="glossary-close" type="button" aria-label="Close">×</button>
                </div>
                <div class="glossary-body"></div>
            </div>`;
        document.body.appendChild(overlay);

        const dialog = overlay.querySelector(".glossary-dialog");
        const titleEl = overlay.querySelector(".glossary-title");
        const bodyEl = overlay.querySelector(".glossary-body");
        const closeBtn = overlay.querySelector(".glossary-close");

        let lastFocused = null;

        function open(key, trigger) {
            const entry = TERMS[key];
            if (!entry) return;
            lastFocused = trigger || null;
            titleEl.textContent = entry.title;
            bodyEl.innerHTML = "";
            for (const para of entry.body) {
                const p = document.createElement("p");
                p.textContent = para;
                bodyEl.appendChild(p);
            }
            overlay.setAttribute("data-open", "true");
            dialog.scrollTop = 0;
            closeBtn.focus();
        }

        function close() {
            if (overlay.getAttribute("data-open") !== "true") return;
            overlay.setAttribute("data-open", "false");
            if (lastFocused && typeof lastFocused.focus === "function") {
                lastFocused.focus();
            }
            lastFocused = null;
        }

        triggers.forEach((trigger) => {
            trigger.setAttribute("type", "button");
            trigger.setAttribute("aria-haspopup", "dialog");
            trigger.addEventListener("click", () => open(trigger.dataset.term, trigger));
        });

        // Click the backdrop (but not the dialog) to dismiss.
        overlay.addEventListener("click", (event) => {
            if (event.target === overlay) close();
        });
        closeBtn.addEventListener("click", close);

        // Escape to dismiss.
        document.addEventListener("keydown", (event) => {
            if (event.key === "Escape") close();
        });
    }

    if (document.readyState === "loading") {
        document.addEventListener("DOMContentLoaded", init);
    } else {
        init();
    }
})();
