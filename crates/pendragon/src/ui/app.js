// Pendragon's pages: overview, VMs (list, new, one VM with its screen and console),
// disks and switches (the virtual network, with its NAT gateways).
//
// Plain JavaScript, no build step. Every element is made with the DOM and every
// piece of data goes in as text (textContent), never as markup, so nothing a VM
// prints or a server says can become script. The pages talk to Pendragon's JSON API
// (docs/pendragon.md) with this launch's token in the X-Pendragon-Token header, and
// to a VM's screen over a WebSocket.
"use strict";

// ---- Token ----

const TOKEN_KEY = "pendragon-token";

// The token arrives in the URL Pendragon printed (?token=...). Keep it for this tab
// and take it out of the address bar, so it is not left in the history or shared by
// copying the address.
function loadToken() {
  const fromUrl = new URLSearchParams(location.search).get("token");
  if (fromUrl && /^[0-9a-f]{64}$/.test(fromUrl)) {
    try {
      sessionStorage.setItem(TOKEN_KEY, fromUrl);
    } catch (e) {
      // Storage blocked: the token lives as long as this page.
    }
    history.replaceState(null, "", location.pathname + location.hash);
    return fromUrl;
  }
  try {
    return sessionStorage.getItem(TOKEN_KEY);
  } catch (e) {
    return null;
  }
}

const token = loadToken();

// ---- Helpers ----

// An element: el("td", {class: "num"}, "text", child, ...). Strings become text nodes.
function el(tag, attrs, ...children) {
  const node = document.createElement(tag);
  for (const [name, value] of Object.entries(attrs || {})) {
    if (value === false || value === null || value === undefined) continue;
    if (name.startsWith("on")) node.addEventListener(name.slice(2), value);
    else if (name === "class") node.className = value;
    else node.setAttribute(name, value === true ? "" : String(value));
  }
  for (const child of children.flat(Infinity)) {
    if (child === null || child === undefined || child === false) continue;
    node.append(child instanceof Node ? child : document.createTextNode(String(child)));
  }
  return node;
}

function notice(text) {
  const box = document.getElementById("notice");
  box.textContent = text || "";
  box.hidden = !text;
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

// A call to the API. Resolves to the reply's JSON; rejects with an Error whose
// message is the server's (and .status, .code).
async function api(method, path, body, signal) {
  const headers = { "X-Pendragon-Token": token || "" };
  const init = { method, headers, cache: "no-store", signal };
  if (body !== undefined) {
    headers["Content-Type"] = "application/json";
    init.body = JSON.stringify(body);
  }
  const reply = await fetch("/api/" + path, init);
  let data = null;
  try {
    data = await reply.json();
  } catch (e) {
    // Not JSON: the status says it all.
  }
  if (!reply.ok) {
    const error = new Error((data && data.error) || reply.status + " " + reply.statusText);
    error.status = reply.status;
    error.code = data && data.code;
    if (reply.status === 403) notice(error.message);
    throw error;
  }
  return data;
}

function toBase64(bytes) {
  let binary = "";
  for (let i = 0; i < bytes.length; i += 0x8000) {
    binary += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
  }
  return btoa(binary);
}

function fromBase64(text) {
  const binary = atob(text);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

function stateCell(vm) {
  return el("span", { class: "state state-" + vm.state }, vm.state_text);
}

function vmLink(id, name) {
  return el("a", { href: "#/vms/" + id }, name);
}

function errorLine() {
  return el("p", { class: "error", hidden: true });
}

function showError(line, error) {
  line.textContent = error ? error.message || String(error) : "";
  line.hidden = !error;
}

// ---- Pages ----

// The page being shown. Each page gets a context; leaving it sets alive to false
// and aborts what it was waiting for.
let page = null;

function leave() {
  if (!page) return;
  page.alive = false;
  page.abort.abort();
  for (const timer of page.timers) clearInterval(timer);
  page = null;
}

function enter(render) {
  leave();
  const ctx = { alive: true, abort: new AbortController(), timers: [] };
  page = ctx;
  const main = document.getElementById("main");
  main.replaceChildren();
  render(main, ctx);
}

// Calls refresh now and every `ms` while the page is shown.
function every(ctx, ms, refresh) {
  refresh();
  ctx.timers.push(setInterval(() => ctx.alive && refresh(), ms));
}

function route() {
  const hash = location.hash.replace(/^#/, "") || "/";
  const [path, query] = hash.split("?");
  const params = new URLSearchParams(query || "");
  const parts = path.split("/").filter(Boolean);
  for (const link of document.querySelectorAll("nav a")) {
    link.classList.toggle("current", link.dataset.page === (parts[0] || "overview"));
  }
  if (!token) {
    notice("This page has no token. Open the URL pendragon printed when it started (it ends in ?token=...).");
  }
  if (parts.length === 0) enter(overviewPage);
  else if (parts[0] === "vms" && parts.length === 1) enter(vmsPage);
  else if (parts[0] === "vms" && parts[1] === "new") enter((m, c) => newVmPage(m, c, params.get("disk")));
  else if (parts[0] === "vms" && /^\d+$/.test(parts[1] || "")) enter((m, c) => vmPage(m, c, parts[1]));
  else if (parts[0] === "disks") enter(disksPage);
  else if (parts[0] === "switches") enter(switchesPage);
  else enter((main) => main.append(el("h1", {}, "No such page"), el("a", { href: "#/" }, "Overview")));
}

// ---- Overview ----

function overviewPage(main, ctx) {
  const body = el("div");
  const error = errorLine();
  main.append(el("h1", {}, "Overview"), error, body);
  every(ctx, 3000, async () => {
    try {
      const o = await api("GET", "overview", undefined, ctx.abort.signal);
      if (!ctx.alive) return;
      showError(error, null);
      body.replaceChildren(
        el("div", { class: "cards" }, serverCard(o.avalon, o.vms, o.ram), lakeCard(o.lake, o.disks)),
        el("p", { class: "muted" }, o.pendragon),
      );
    } catch (e) {
      if (ctx.alive) showError(error, e);
    }
  });
}

function health(server) {
  return [
    el("p", {}, el("span", { class: server.ok ? "ok" : "down" }, server.ok ? "up" : "down"), " at ", server.addr),
    server.ok ? el("p", { class: "muted" }, server.server) : el("p", { class: "error" }, server.error),
  ];
}

function serverCard(avalon, vms, ram) {
  const card = el("div", { class: "card" }, el("h2", {}, "Avalon"), health(avalon));
  if (ram) {
    const share = ram.budget > 0 ? Math.min(1, ram.used / ram.budget) : 0;
    const fill = el("div");
    fill.style.width = (share * 100).toFixed(1) + "%";
    card.append(
      el("p", {}, "RAM in use: ", ram.used_text, " of ", ram.budget_text),
      el("div", { class: share >= 0.9 ? "bar full" : "bar" }, fill),
    );
  }
  if (vms) {
    card.append(
      el(
        "p",
        {},
        el("a", { href: "#/vms" }, vms.total + " VMs"),
        ": " + vms.running + " running, " + vms.paused + " paused, " + vms.created + " created, " + vms.ended + " ended",
      ),
    );
  }
  return card;
}

function lakeCard(lake, disks) {
  const card = el("div", { class: "card" }, el("h2", {}, "Lake"), health(lake));
  if (disks) {
    card.append(
      el("p", {}, el("a", { href: "#/disks" }, disks.total + " disks"), ", " + disks.attached + " attached"),
      el("p", { class: "muted" }, disks.bytes_text + " in all"),
    );
  }
  return card;
}

// ---- VMs ----

const CONFIRM = {
  stop: "Stop VM {vm}? A stopped VM cannot be started again, and it releases its disks.",
  destroy: "Destroy VM {vm}? Its console and state are gone for good.",
};

// The buttons for a VM's allowed actions; `after` runs once one is done.
function actionButtons(vm, error, after) {
  return vm.actions.map((action) =>
    el(
      "button",
      {
        class: action === "destroy" || action === "stop" ? "danger" : "",
        onclick: async (event) => {
          const question = CONFIRM[action];
          if (question && !confirm(question.replace("{vm}", vm.id + " (" + vm.name + ")"))) return;
          event.target.disabled = true;
          try {
            await api("POST", "vms/" + vm.id + "/" + action);
            showError(error, null);
          } catch (e) {
            showError(error, e);
          }
          after(action);
        },
      },
      action,
    ),
  );
}

function diskList(vm) {
  if (vm.disks.length === 0) return el("span", { class: "muted" }, "none");
  return vm.disks.map((d) => el("div", {}, d.source, d.read_only ? " (read-only)" : ""));
}

function vmsPage(main, ctx) {
  const error = errorLine();
  const table = el("div");
  main.append(
    el("h1", {}, "VMs"),
    el("p", {}, el("button", { class: "primary", onclick: () => (location.hash = "#/vms/new") }, "New VM")),
    error,
    table,
  );
  const refresh = async () => {
    try {
      const list = await api("GET", "vms", undefined, ctx.abort.signal);
      if (!ctx.alive) return;
      if (list.vms.length === 0) {
        table.replaceChildren(el("p", { class: "muted" }, "No VMs on Avalon at " + list.avalon + "."));
        return;
      }
      table.replaceChildren(
        el(
          "table",
          {},
          el("tr", {}, ["ID", "Name", "State", "RAM", "Clock", "Disks", ""].map((h) => el("th", {}, h))),
          list.vms.map((vm) =>
            el(
              "tr",
              {},
              el("td", { class: "num" }, vm.id),
              el("td", {}, vmLink(vm.id, vm.name)),
              el("td", {}, stateCell(vm)),
              el("td", { class: "num" }, vm.ram_text),
              el("td", {}, vm.clock_text),
              el("td", {}, diskList(vm)),
              el("td", {}, actionButtons(vm, error, refresh)),
            ),
          ),
        ),
      );
    } catch (e) {
      if (ctx.alive) showError(error, e);
    }
  };
  every(ctx, 2000, refresh);
}

function newVmPage(main, ctx, disk) {
  const error = errorLine();
  const lakeImages = el("datalist", { id: "lake-images" });
  const disks = el("div");
  let lake = null;

  const nets = el("div");
  const switchNames = el("datalist", { id: "switch-names" });
  const addNet = () => {
    const row = el(
      "div",
      { class: "disk-row net-row" },
      el("input", { type: "text", class: "net-switch", list: "switch-names", placeholder: "switch (empty: unplugged)" }),
      el("input", { type: "text", class: "net-mac", placeholder: "MAC (empty: one is made up)", autocomplete: "off" }),
      el("button", { type: "button", onclick: () => row.remove() }, "remove"),
    );
    nets.append(row);
  };

  const addDisk = (source) => {
    const row = el(
      "div",
      { class: "disk-row" },
      el("input", {
        type: "text",
        class: "disk-source",
        list: "lake-images",
        placeholder: "lake://host:port/name, or a file on Avalon's host",
        value: source || "",
      }),
      el("label", { class: "inline" }, el("input", { type: "checkbox", class: "disk-ro" }), " read-only"),
      el("button", { type: "button", onclick: () => row.remove() }, "remove"),
    );
    disks.append(row);
  };

  const field = (label, input, hint) => el("label", {}, el("span", {}, label), input, hint ? el("span", { class: "muted" }, " " + hint) : null);
  const name = el("input", { type: "text", required: true, placeholder: "web", autocomplete: "off" });
  const ram = el("input", { type: "text", value: "4" });
  const clock = el("input", { type: "text", placeholder: "full speed" });
  const harts = el("input", { type: "text", value: "1" });
  const program = el("input", { type: "file", accept: ".elf,application/octet-stream" });
  const start = el("input", { type: "checkbox", checked: true });
  const submit = el("button", { type: "submit", class: "primary" }, "Create");

  // What the VM boots: Camelot from a copy of Lake's Camelot image of its own (the
  // choice when Lake has that image), or an uploaded program (an ELF), which may be
  // left out when there is a disk to boot from instead.
  const bootCamelot = el("input", { type: "radio", name: "boot", value: "camelot" });
  const bootProgram = el("input", { type: "radio", name: "boot", value: "program", checked: true });
  const camelotHint = el("span", { class: "muted" }, " Lake's image is copied to a disk of the VM's own, NAME-disk0");
  const programField = field("Program", program, "an Excalibur ELF executable; without one the VM boots from its first disk");
  const showBoot = () => {
    programField.hidden = bootCamelot.checked;
  };
  bootCamelot.addEventListener("change", showBoot);
  bootProgram.addEventListener("change", showBoot);
  const boot = el(
    "fieldset",
    { class: "boot" },
    el("legend", {}, "Boots"),
    el("label", { class: "inline" }, bootCamelot, " Camelot (boot from disk)"),
    camelotHint,
    el("label", { class: "inline" }, bootProgram, " Program (ELF)"),
  );

  const form = el(
    "form",
    { class: "panel" },
    field("Name", name, "lowercase letters, digits, '.', '_' and '-'"),
    field("RAM", ram, "MiB, or 512M, 16G; 1 MiB to 1 TiB, an RV32 program's VM at most 2 GiB"),
    field("Clock", clock, "e.g. 10MHz, 500kHz, 5Hz; empty runs at full speed"),
    field("Harts", harts, "cores, 1 to 8; each runs at the clock"),
    boot,
    programField,
    el("h2", {}, "Disks"),
    disks,
    el("button", { type: "button", onclick: () => addDisk("") }, "Add a disk"),
    lakeImages,
    el("h2", {}, "Network cards"),
    nets,
    el("button", { type: "button", onclick: addNet }, "Add a network card"),
    switchNames,
    el("label", {}, start, " start it once created"),
    submit,
  );
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    submit.disabled = true;
    try {
      const request = {
        name: name.value.trim(),
        ram: ram.value,
        clock: clock.value,
        harts: harts.value,
        start: start.checked,
        camelot: bootCamelot.checked,
        disks: [...disks.querySelectorAll(".disk-row")]
          .map((row) => ({
            source: row.querySelector(".disk-source").value.trim(),
            read_only: row.querySelector(".disk-ro").checked,
          }))
          .filter((d) => d.source !== ""),
        nets: [...nets.querySelectorAll(".net-row")].map((row) => ({
          switch: row.querySelector(".net-switch").value.trim(),
          mac: row.querySelector(".net-mac").value.trim() || null,
        })),
      };
      const file = program.files[0];
      if (!request.camelot && file) {
        request.program = toBase64(new Uint8Array(await file.arrayBuffer()));
      } else if (!request.camelot && request.disks.length === 0) {
        throw new Error("Choose the program to run (an ELF file), or a disk to boot from.");
      }
      const created = await api("POST", "vms", request);
      location.hash = "#/vms/" + created.id;
    } catch (e) {
      showError(error, e);
      submit.disabled = false;
    }
  });
  main.append(el("h1", {}, "New VM"), error, form);

  // Offer Lake's images as disk sources, and prefill the one asked for (Attach on
  // the disks page). Camelot is the first choice when Lake has its image and no disk
  // was asked for.
  showBoot();
  api("GET", "disks", undefined, ctx.abort.signal)
    .then((list) => {
      lake = list.lake;
      for (const d of list.disks) lakeImages.append(el("option", { value: "lake://" + lake + "/" + d.name }));
      if (disk) addDisk("lake://" + lake + "/" + disk);
      const hasImage = list.disks.some((d) => d.name === list.camelot_image);
      if (hasImage && !disk) {
        bootCamelot.checked = true;
      } else if (!hasImage) {
        camelotHint.textContent = " Lake has no image named " + list.camelot_image + " to copy: start the platform with cargo xtask up";
      }
      showBoot();
    })
    .catch(() => {
      if (disk && ctx.alive) addDisk("");
    });
  // And Avalon's switches as places to plug the cards in.
  api("GET", "switches", undefined, ctx.abort.signal)
    .then((list) => {
      for (const s of list.switches) switchNames.append(el("option", { value: s.name }));
    })
    .catch(() => {});
}

function vmPage(main, ctx, id) {
  const error = errorLine();
  const title = el("h1", {}, "VM " + id);
  const facts = el("div", { class: "panel" });
  const actions = el("p");
  const screen = el("pre", { class: "console", tabindex: 0 });
  const status = el("p", { class: "muted" });
  main.append(
    title,
    error,
    actions,
    facts,
    el("h2", {}, "Screen"),
    ...screenView(ctx, id),
    el("h2", {}, "Console"),
    screen,
    consoleInput(ctx, id, error),
    status,
  );

  const refresh = async () => {
    try {
      const vm = await api("GET", "vms/" + id, undefined, ctx.abort.signal);
      if (!ctx.alive) return;
      title.textContent = "VM " + vm.id + " (" + vm.name + ")";
      actions.replaceChildren(...actionButtons(vm, error, (action) => {
        if (action === "destroy") location.hash = "#/vms";
        else refresh();
      }));
      facts.replaceChildren(vmFacts(vm));
    } catch (e) {
      if (!ctx.alive) return;
      showError(error, e);
      if (e.status === 404) {
        leave();
        facts.replaceChildren(el("a", { href: "#/vms" }, "Back to the VMs"));
      }
    }
  };
  every(ctx, 2000, refresh);
  consoleLoop(ctx, id, screen, status);
}

function vmFacts(vm) {
  const rows = [
    ["State", stateCell(vm)],
    ["RAM", vm.ram_text],
    ["Clock", vm.clock_hz === null ? "full speed (reports " + vm.nominal_text + " to the guest)" : vm.clock_text + " (real time)"],
    ["Harts", String(vm.harts)],
    ["Steps", String(vm.steps)],
    ["Disks", el("div", {}, diskList(vm))],
    ["Devices", el("div", {}, vm.devices.map((d) => el("div", {}, d.name + "  " + d.base + " + " + d.size + (d.irq ? "  irq " + d.irq : ""))))],
  ];
  return el("dl", { class: "facts" }, rows.map(([k, v]) => [el("dt", {}, k), el("dd", {}, v)]));
}

// What Enter adds to a line: the guest decides which it wants (Grail's terminal sends
// CR for the key, a pipe LF).
const ENDINGS = { lf: "\n", cr: "\r", none: "" };

function consoleInput(ctx, id, error) {
  const text = el("input", { type: "text", placeholder: "type here, Enter sends", autocomplete: "off", spellcheck: "false" });
  const ending = el(
    "select",
    { title: "what Enter adds" },
    el("option", { value: "lf" }, "LF"),
    el("option", { value: "cr" }, "CR"),
    el("option", { value: "none" }, "nothing"),
  );
  const send = async (data) => {
    try {
      await api("POST", "vms/" + id + "/console", { data });
      showError(error, null);
    } catch (e) {
      if (ctx.alive) showError(error, e);
    }
  };
  const form = el(
    "form",
    { class: "console-input" },
    text,
    ending,
    el("button", { type: "submit" }, "Send"),
    el("button", { type: "button", title: "end of transmission", onclick: () => send("\x04") }, "Ctrl-D"),
  );
  form.addEventListener("submit", (event) => {
    event.preventDefault();
    const data = text.value + ENDINGS[ending.value];
    text.value = "";
    if (data !== "") send(data);
  });
  return form;
}

// The most console text kept on the page.
const SCREEN_MAX = 256 * 1024;

// Long-polls the console: each request waits on Pendragon until there is output.
async function consoleLoop(ctx, id, screen, status) {
  let tap = null;
  let next = 0;
  let text = "";
  let decoder = new TextDecoder();
  while (ctx.alive) {
    let r;
    try {
      const query = "from=" + next + (tap === null ? "" : "&tap=" + tap) + "&wait=15000";
      r = await api("GET", "vms/" + id + "/console?" + query, undefined, ctx.abort.signal);
    } catch (e) {
      if (!ctx.alive) return;
      status.textContent = "console: " + e.message;
      await sleep(e.status === 404 ? 10000 : 2000);
      continue;
    }
    if (!ctx.alive) return;
    if (r.reset) {
      text = "";
      decoder = new TextDecoder();
    } else if (tap !== null && r.from > next) {
      text += "\n[... earlier output skipped ...]\n";
    }
    tap = r.tap;
    next = r.next;
    const atBottom = screen.scrollTop + screen.clientHeight >= screen.scrollHeight - 4;
    text = terminal(text, decoder.decode(fromBase64(r.data), { stream: true }));
    if (text.length > SCREEN_MAX) text = text.slice(text.length - SCREEN_MAX);
    screen.textContent = text;
    if (atBottom) screen.scrollTop = screen.scrollHeight;
    status.textContent = r.ended ? "console ended: " + r.ended : "";
    if (r.ended) return;
  }
}

// Characters that show: newline, tab, and anything from space up but DEL.
const PRINTABLE = (c) => c === "\n" || c === "\t" || (c >= " " && c !== "\x7f");

// A little of what a terminal does with output: backspace erases, escape sequences
// (colours, cursor moves) and other control characters are dropped.
function terminal(text, output) {
  output = output.replace(/\x1b\[[0-9;?]*[ -\/]*[@-~]/g, "").replace(/\x1b[@-_]?/g, "");
  let out = text;
  for (const c of output) {
    if (c === "\b") {
      if (out.length > 0 && !out.endsWith("\n")) out = out.slice(0, -1);
    } else if (PRINTABLE(c)) {
      out += c;
    }
  }
  return out;
}

// ---- Screen ----

// The VM's screen in a canvas, with its keyboard and mouse, over a WebSocket on which
// Pendragon is the VM's viewer (docs/pendragon.md, "Screens"). Pendragon sends the
// screen's size (again on every mode change), rectangles of RGBA pixels and the
// guest's cursor, its image and its place apart from the frames; the page
// sends what the browser reports, as it reports it: keys by KeyboardEvent.code and
// .key (Pendragon has the key table, Grail's HID usages), Pointer Events in screen
// pixels, wheel deltas. A browser lets a page set no header on a WebSocket, so the
// token goes in a subprotocol it offers.
function screenView(ctx, id) {
  const canvas = el("canvas", { class: "screen", width: 640, height: 480, tabindex: 0 });
  const status = el("p", { class: "muted" }, "screen: connecting");
  const draw = canvas.getContext("2d", { alpha: false });
  // The guest's cursor, on a transparent canvas of the screen's size laid over it (the
  // browser blends the two): moving the cursor redraws two small rectangles of this
  // one and no frame. Its image is straight RGBA, as ImageData takes it (Pendragon
  // divided the guest's premultiplied colours by their alpha). The pointer's events
  // go through it to the screen.
  const overlay = el("canvas", { class: "cursor", width: 640, height: 480 });
  const pen = overlay.getContext("2d");
  const cursor = { image: null, visible: false, x: 0, y: 0 };
  // Puts the cursor where it is now: clears the rectangle it was drawn in, draws it
  // anew if it shows. putImageData replaces pixels and clips at the canvas's edges.
  let drawn = null;
  const drawCursor = () => {
    if (drawn) pen.clearRect(drawn.x, drawn.y, drawn.width, drawn.height);
    drawn = null;
    if (cursor.image && cursor.visible) {
      pen.putImageData(cursor.image, cursor.x, cursor.y);
      drawn = { x: cursor.x, y: cursor.y, width: cursor.image.width, height: cursor.image.height };
    }
  };
  let ws = null;
  let ended = false;
  let retry = null;
  let size = "";
  let state = "";
  const show = () => {
    const typing = document.activeElement === canvas;
    status.textContent =
      "screen: " + size + (state ? ", " + state : "") +
      (ended ? "" : typing ? ". Keys go to the VM; click outside the screen to stop" : ". Click the screen to type into the VM");
  };
  const send = (msg) => {
    if (ws && ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify(msg));
  };
  const altGr = altGrFilter(send);

  const open = () => {
    if (!ctx.alive) return;
    const scheme = location.protocol === "https:" ? "wss://" : "ws://";
    ws = new WebSocket(scheme + location.host + "/api/vms/" + encodeURIComponent(id) + "/screen", [
      "pendragon.screen",
      "pendragon.token." + (token || ""),
    ]);
    ws.binaryType = "arraybuffer";
    ws.addEventListener("message", (event) => {
      if (typeof event.data !== "string") {
        // A rectangle: x, y, width, height (u32, little-endian), then RGBA pixels.
        const head = new DataView(event.data, 0, 16);
        const w = head.getUint32(8, true);
        const h = head.getUint32(12, true);
        const pixels = new Uint8ClampedArray(event.data, 16, w * h * 4);
        draw.putImageData(new ImageData(pixels, w, h), head.getUint32(0, true), head.getUint32(4, true));
        return;
      }
      const msg = JSON.parse(event.data);
      if (msg.type === "screen") {
        // A new screen (the first, or the guest changed modes): black, at its size.
        canvas.width = msg.width;
        canvas.height = msg.height;
        draw.fillStyle = "#000";
        draw.fillRect(0, 0, msg.width, msg.height);
        size = msg.width + " × " + msg.height;
        // The cursor stays where it was, on the new screen.
        overlay.width = msg.width;
        overlay.height = msg.height;
        drawn = null;
        drawCursor();
      } else if (msg.type === "cursor_shape") {
        // The cursor's image from now on, also while it is hidden.
        const pixels = new Uint8ClampedArray(fromBase64(msg.pixels).buffer);
        cursor.image = new ImageData(pixels, msg.width, msg.height);
        drawCursor();
        return;
      } else if (msg.type === "cursor_position") {
        cursor.visible = msg.visible;
        cursor.x = msg.x;
        cursor.y = msg.y;
        drawCursor();
        return;
      } else if (msg.type === "state") {
        state = msg.state_text;
      } else if (msg.type === "end") {
        ended = true;
        state = "ended: " + msg.reason;
      }
      show();
    });
    ws.addEventListener("close", () => {
      altGr.drop();
      if (!ctx.alive || ended) return;
      // Pendragon restarted, Avalon away, no such VM: the facts above say which.
      status.textContent = "screen: not connected; trying again";
      // The next connection starts with no cursor: Avalon sends it again on attaching.
      cursor.image = null;
      cursor.visible = false;
      drawCursor();
      retry = setTimeout(open, 3000);
    });
  };

  // Keys, while the canvas has the focus. Every key's default action is stopped (Tab,
  // Backspace, F5 and the like are the guest's); the few the browser keeps for itself
  // (Ctrl+W) never reach the page.
  const key = (event, down) => {
    event.preventDefault();
    altGr.key(event, {
      type: "key",
      down,
      code: event.code,
      key: event.key,
      shift: event.shiftKey,
      ctrl: event.ctrlKey,
      alt: event.altKey,
    });
  };
  canvas.addEventListener("keydown", (event) => key(event, true));
  canvas.addEventListener("keyup", (event) => key(event, false));
  // Key-ups go elsewhere once the canvas loses the focus (to another element, another
  // window, a hidden tab): Pendragon releases every key and button the page held, as
  // Grail's window does.
  const release = () => {
    altGr.drop();
    send({ type: "release" });
    show();
  };
  canvas.addEventListener("blur", release);
  canvas.addEventListener("focus", show);
  const hidden = () => {
    if (document.visibilityState === "hidden") release();
  };
  document.addEventListener("visibilitychange", hidden);

  // The pointer, in screen pixels: the canvas may be drawn smaller than the screen.
  // Pointer capture keeps a drag that leaves the canvas going to it, positions past
  // its edges included (Pendragon clamps them).
  const pointer = (event) => {
    const box = canvas.getBoundingClientRect();
    if (box.width === 0 || box.height === 0) return;
    send({
      type: "pointer",
      x: Math.floor(((event.clientX - box.left) * canvas.width) / box.width),
      y: Math.floor(((event.clientY - box.top) * canvas.height) / box.height),
      buttons: event.buttons & 7,
    });
  };
  canvas.addEventListener("pointerdown", (event) => {
    // No text selection, no middle-button scrolling: the button is the guest's.
    event.preventDefault();
    canvas.focus();
    canvas.setPointerCapture(event.pointerId);
    pointer(event);
  });
  canvas.addEventListener("pointermove", pointer);
  canvas.addEventListener("pointerup", pointer);
  // The browser took the pointer away (a touch became a scroll, say): the buttons
  // are up as far as the guest is concerned.
  canvas.addEventListener("pointercancel", () => send({ type: "pointer", buttons: 0 }));
  canvas.addEventListener("contextmenu", (event) => event.preventDefault());
  canvas.addEventListener(
    "wheel",
    (event) => {
      event.preventDefault();
      send({ type: "wheel", dy: event.deltaY, mode: event.deltaMode });
    },
    { passive: false },
  );

  ctx.abort.signal.addEventListener("abort", () => {
    clearTimeout(retry);
    altGr.drop();
    document.removeEventListener("visibilitychange", hidden);
    if (ws) ws.close(1000, "the page left");
  });
  open();
  return [el("div", { class: "screen-box" }, canvas, overlay), status];
}

// Windows reports AltGr as two keys pressed together: a Left Ctrl that is not there,
// and Right Alt. As noVNC does (its keyboard.js), a Left Ctrl press on Windows is held
// back for a moment: a Right Alt press within 50 ms makes it the fake one, which is
// dropped (its key-up, of a key Pendragon never saw go down, reports nothing);
// anything else, or 100 ms, sends it.
function altGrFilter(send) {
  const platform = (navigator.userAgentData && navigator.userAgentData.platform) || navigator.platform || "";
  const windows = /^Win/.test(platform);
  let held = null;
  const flush = () => {
    if (!held) return;
    clearTimeout(held.timer);
    const msg = held.msg;
    held = null;
    send(msg);
  };
  return {
    key(event, msg) {
      if (held) {
        if (msg.down && event.code === "AltRight" && event.timeStamp - held.time < 50) {
          clearTimeout(held.timer);
          held = null;
        } else {
          flush();
        }
      }
      if (windows && msg.down && event.code === "ControlLeft") {
        held = { msg, time: event.timeStamp, timer: setTimeout(flush, 100) };
        return;
      }
      send(msg);
    },
    // The focus went, and Pendragon releases what was sent: a Ctrl not yet sent is
    // not sent.
    drop() {
      if (held) clearTimeout(held.timer);
      held = null;
    },
  };
}

// ---- Disks ----

function disksPage(main, ctx) {
  const error = errorLine();
  const table = el("div");
  const name = el("input", { type: "text", required: true, placeholder: "data", autocomplete: "off" });
  const size = el("input", { type: "text", required: true, placeholder: "64MiB" });
  const create = el(
    "form",
    { class: "panel" },
    el("label", {}, el("span", {}, "Name"), name),
    el("label", {}, el("span", {}, "Size"), size, el("span", { class: "muted" }, " 64MiB, 512K, 1G, 2048s (sectors)")),
    el("button", { type: "submit", class: "primary" }, "Create disk"),
  );
  main.append(el("h1", {}, "Disks"), create, error, table);

  const refresh = async () => {
    try {
      const list = await api("GET", "disks", undefined, ctx.abort.signal);
      if (!ctx.alive) return;
      showError(error, list.vms_error ? new Error("Which VMs use each disk is unknown: " + list.vms_error) : null);
      if (list.disks.length === 0) {
        table.replaceChildren(el("p", { class: "muted" }, "No disks on Lake at " + list.lake + "."));
        return;
      }
      table.replaceChildren(
        el(
          "table",
          {},
          el("tr", {}, ["Name", "Size", "Sectors", "Attached", ""].map((h) => el("th", {}, h))),
          list.disks.map((d) => diskRow(d, error, refresh)),
        ),
      );
    } catch (e) {
      if (ctx.alive) showError(error, e);
    }
  };
  create.addEventListener("submit", async (event) => {
    event.preventDefault();
    try {
      await api("POST", "disks", { name: name.value.trim(), size: size.value.trim() });
      name.value = "";
      size.value = "";
      showError(error, null);
    } catch (e) {
      showError(error, e);
    }
    refresh();
  });
  every(ctx, 3000, refresh);
}

function attachedText(d) {
  const parts = [];
  if (d.writer) parts.push("read-write");
  if (d.readers === 1) parts.push("1 reader");
  if (d.readers > 1) parts.push(d.readers + " readers");
  const cell = el("td", {}, parts.length ? parts.join(", ") : el("span", { class: "muted" }, "no"));
  if (d.attached_to) {
    for (const vm of d.attached_to) {
      cell.append(el("div", {}, "VM ", vmLink(vm.id, vm.id + " (" + vm.name + ")"), vm.read_only ? ", read-only" : ", read-write"));
    }
    if (d.attached && d.attached_to.length === 0) {
      cell.append(el("div", { class: "muted" }, "by a client of Lake that Pendragon does not manage"));
    }
  }
  return cell;
}

function diskRow(d, error, refresh) {
  const holders = d.attached_to || [];
  const detach = async (button) => {
    const names = holders.map((vm) => vm.id + " (" + vm.name + ")").join(", ");
    const question =
      "Detach disk " + d.name + "? A VM's disks are part of its hardware: detaching stops VM " + names +
      ", which cannot be started again.";
    if (!confirm(question)) return;
    button.disabled = true;
    try {
      const r = await api("POST", "disks/" + encodeURIComponent(d.name) + "/detach");
      showError(error, r.released ? null : new Error("The VMs stopped, but Lake does not show the disk released yet."));
    } catch (e) {
      showError(error, e);
    }
    refresh();
  };
  const remove = async (button) => {
    if (!confirm("Delete disk " + d.name + " and everything on it?")) return;
    button.disabled = true;
    try {
      await api("DELETE", "disks/" + encodeURIComponent(d.name));
      showError(error, null);
    } catch (e) {
      showError(error, e);
    }
    refresh();
  };
  return el(
    "tr",
    {},
    el("td", {}, d.name),
    el("td", { class: "num" }, d.size_text),
    el("td", { class: "num" }, d.sectors),
    attachedText(d),
    el(
      "td",
      {},
      el("button", { title: "create a VM with this disk", onclick: () => (location.hash = "#/vms/new?disk=" + encodeURIComponent(d.name)) }, "attach"),
      el("button", { disabled: holders.length === 0, title: "stop the VMs that hold it", onclick: (e) => detach(e.target) }, "detach"),
      el("button", { class: "danger", disabled: d.attached, title: d.attached ? "detach it first" : "", onclick: (e) => remove(e.target) }, "delete"),
    ),
  );
}

// ---- Switches ----

function switchesPage(main, ctx) {
  const error = errorLine();
  const list = el("div");

  // Create a switch, with or without a NAT gateway and its forwarded ports.
  const name = el("input", { type: "text", required: true, placeholder: "lan", autocomplete: "off" });
  const nat = el("input", { type: "checkbox" });
  const hostfwd = el("textarea", {
    rows: 3,
    cols: 40,
    placeholder: "tcp::8080-:80\nudp:127.0.0.1:5353-10.0.2.16:53",
    spellcheck: "false",
  });
  const create = el(
    "form",
    { class: "panel" },
    el("label", {}, el("span", {}, "Name"), name, el("span", { class: "muted" }, " lowercase letters, digits, '.', '_' and '-'")),
    el("label", {}, nat, " give it a NAT gateway (10.0.2.0/24: DHCP, DNS, and the host at 10.0.2.2)"),
    el(
      "label",
      {},
      el("span", {}, "Forwarded ports"),
      hostfwd,
      el("span", { class: "muted" }, " one per line, [tcp|udp]:[host addr]:port-[guest addr]:port; the guest is 10.0.2.15 unless named"),
    ),
    el("button", { type: "submit", class: "primary" }, "Create switch"),
  );
  create.addEventListener("submit", async (event) => {
    event.preventDefault();
    try {
      await api("POST", "switches", { name: name.value.trim(), nat: nat.checked, hostfwd: lines(hostfwd.value) });
      name.value = "";
      nat.checked = false;
      hostfwd.value = "";
      showError(error, null);
    } catch (e) {
      showError(error, e);
    }
    refresh();
  });

  // Plug a VM's card into a switch. The choices are rebuilt on each refresh, keeping
  // what was picked.
  const plugSwitch = el("select");
  const plugVm = el("select");
  const plugCard = el("select");
  const plug = el(
    "form",
    { class: "panel console-input" },
    el("span", {}, "Plug card "),
    plugCard,
    el("span", {}, " of VM "),
    plugVm,
    el("span", {}, " into "),
    plugSwitch,
    el("button", { type: "submit" }, "Plug in"),
  );
  let vms = [];
  const fillCards = () => {
    const vm = vms.find((v) => String(v.id) === plugVm.value);
    options(
      plugCard,
      (vm ? vm.nets : []).map((n) => [String(n.card), n.card + " (" + n.mac + (n.switch ? ", on " + n.switch : "") + ")"]),
    );
  };
  plugVm.addEventListener("change", fillCards);
  plug.addEventListener("submit", async (event) => {
    event.preventDefault();
    try {
      if (!plugSwitch.value || !plugVm.value || !plugCard.value) throw new Error("Choose a switch, a VM and one of its cards.");
      await api("POST", "switches/" + encodeURIComponent(plugSwitch.value) + "/attach", {
        vm: Number(plugVm.value),
        card: Number(plugCard.value),
      });
      showError(error, null);
    } catch (e) {
      showError(error, e);
    }
    refresh();
  });

  main.append(el("h1", {}, "Switches"), create, plug, error, list);

  const refresh = async () => {
    try {
      const [switches, vmList] = await Promise.all([
        api("GET", "switches", undefined, ctx.abort.signal),
        api("GET", "vms", undefined, ctx.abort.signal).catch(() => ({ vms: [] })),
      ]);
      if (!ctx.alive) return;
      if (switches.vms_error) showError(error, new Error("Which VMs own each card is unknown: " + switches.vms_error));
      vms = vmList.vms.filter((v) => v.nets.length > 0);
      options(plugSwitch, switches.switches.map((s) => [s.name, s.name]));
      options(plugVm, vms.map((v) => [String(v.id), v.id + " (" + v.name + ")"]));
      fillCards();
      if (switches.switches.length === 0) {
        list.replaceChildren(el("p", { class: "muted" }, "No switches on Avalon at " + switches.avalon + "."));
        return;
      }
      list.replaceChildren(...switches.switches.map((s) => switchPanel(s, error, refresh)));
    } catch (e) {
      if (ctx.alive) showError(error, e);
    }
  };
  every(ctx, 2000, refresh);
}

// The non-empty lines of a text box.
function lines(text) {
  return text
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line !== "");
}

// Replaces a select's options with [value, label] pairs, keeping the chosen value if
// it is still there.
function options(select, pairs) {
  const chosen = select.value;
  select.replaceChildren(...pairs.map(([value, label]) => el("option", { value }, label)));
  if (pairs.some(([value]) => value === chosen)) select.value = chosen;
}

function switchPanel(s, error, refresh) {
  const path = "switches/" + encodeURIComponent(s.name);
  const run = async (button, method, suffix, body, question) => {
    if (question && !confirm(question)) return;
    button.disabled = true;
    try {
      await api(method, path + suffix, body);
      showError(error, null);
    } catch (e) {
      showError(error, e);
    }
    refresh();
  };
  const remove = el(
    "button",
    {
      class: "danger",
      onclick: (e) =>
        run(e.target, "DELETE", "", undefined, "Delete switch " + s.name + "? Its cards are unplugged and its gateway's host ports closed."),
    },
    "delete",
  );
  const natButton = s.nat
    ? el(
        "button",
        {
          onclick: (e) =>
            run(e.target, "DELETE", "/nat", undefined, "Take switch " + s.name + "'s NAT gateway away? Its connections and forwarded ports close."),
        },
        "remove NAT",
      )
    : el("button", { onclick: (e) => natForm(e.target) }, "add NAT");
  const natArea = el("div");
  // Adding a gateway asks for its forwarded ports first.
  const natForm = (button) => {
    button.disabled = true;
    const rules = el("textarea", { rows: 2, cols: 40, placeholder: "tcp::8080-:80", spellcheck: "false" });
    const form = el(
      "form",
      {},
      el("label", {}, el("span", {}, "Forwarded ports"), rules, el("span", { class: "muted" }, " one per line, may be empty")),
      el("button", { type: "submit", class: "primary" }, "Add NAT gateway"),
    );
    form.addEventListener("submit", (event) => {
      event.preventDefault();
      run(form.querySelector("button"), "POST", "/nat", { hostfwd: lines(rules.value) });
    });
    natArea.replaceChildren(form);
  };

  const panel = el(
    "div",
    { class: "panel" },
    el("div", { class: "switch-head" }, el("h2", {}, s.name), natButton, remove),
    s.capture ? el("p", { class: "muted" }, "capturing to " + s.capture) : null,
    natArea,
  );
  if (s.nat) panel.append(natFacts(s.nat));
  else natArea.append(el("p", { class: "muted" }, "No NAT gateway: the machines on it only reach each other."));

  if (s.ports.length === 0) {
    panel.append(el("p", { class: "muted" }, "Nothing is plugged in."));
    return panel;
  }
  panel.append(
    el(
      "table",
      {},
      el("tr", {}, ["Port", "MAC", "In", "Out", "Dropped", ""].map((h) => el("th", {}, h))),
      s.ports.map((p) =>
        el(
          "tr",
          {},
          el("td", {}, portOwner(p)),
          el("td", { class: "mono" }, p.mac),
          el("td", { class: "num" }, p.in_frames + " frames, " + p.in_bytes + " bytes"),
          el("td", { class: "num" }, p.out_frames + " frames, " + p.out_bytes + " bytes"),
          el("td", { class: "num" }, p.dropped),
          el(
            "td",
            {},
            p.kind === "card"
              ? el(
                  "button",
                  {
                    title: "unplug the card from this switch",
                    onclick: (e) => run(e.target, "POST", "/detach", { vm: p.vm.id, card: p.card }),
                  },
                  "unplug",
                )
              : null,
          ),
        ),
      ),
    ),
  );
  return panel;
}

function portOwner(p) {
  if (p.kind === "gateway") return "NAT gateway";
  const who = p.vm.name === null ? "VM " + p.vm.id : vmLink(p.vm.id, "VM " + p.vm.id + " (" + p.vm.name + ")");
  return el("span", {}, who, ", card " + p.card);
}

function natFacts(nat) {
  const rows = [
    ["Network", nat.network + ", gateway and host " + nat.gateway + ", DNS " + nat.dns],
    [
      "Forwarded",
      nat.hostfwd.length === 0 ? el("span", { class: "muted" }, "none") : el("div", {}, nat.hostfwd.map((r) => el("div", { class: "mono" }, r))),
    ],
    [
      "Leases",
      nat.leases.length === 0
        ? el("span", { class: "muted" }, "none yet")
        : el(
            "div",
            {},
            nat.leases.map((l) =>
              el("div", {}, el("span", { class: "mono" }, l.addr + "  " + l.mac), l.vm ? [" ", vmLink(l.vm.id, "VM " + l.vm.id + " (" + l.vm.name + ")")] : null),
            ),
          ),
    ],
  ];
  return el("dl", { class: "facts" }, rows.map(([k, v]) => [el("dt", {}, k), el("dd", {}, v)]));
}

// ---- Start ----

window.addEventListener("hashchange", route);
route();
