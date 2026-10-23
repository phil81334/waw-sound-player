import { Channel, invoke } from "@tauri-apps/api/core";

const element = (id) => document.getElementById(id);
const desktop = "__TAURI_INTERNALS__" in window;
const ROW_HEIGHT = 52;
const CARD_HEIGHT = 84;
const CARD_GAP = 8;
let previousLayout = null;
let entries = [];
let filtered = [];
let selected = null;
let selection = null;
let sourceIndex = 0;
let selectionRequest = 0;
let loading = false;
let scanning = false;
let playing = false;
let muted = false;
let seeking = false;
let polling = false;
let frame = 0;
const rows = new Map();
const failures = new Map();

function message(text, error = false) {
  element("message").textContent = text;
  element("message").classList.toggle("error", error);
  element("status").textContent = error ? "Error" : scanning ? "Reading archives…" : "Ready";
}

function bytes(value) {
  if (value < 1024) { return `${value} B`; }
  if (value < 1024 * 1024) { return `${(value / 1024).toFixed(1)} KiB`; }
  return `${(value / (1024 * 1024)).toFixed(1)} MiB`;
}

function time(value) {
  const seconds = Math.max(0, Number(value) || 0);
  return `${Math.floor(seconds / 60)}:${(seconds % 60).toFixed(2).padStart(5, "0")}`;
}

function option(select, value, label) {
  const item = document.createElement("option");
  item.value = value;
  item.textContent = label;
  select.append(item);
}

function scheduleRows() {
  if (frame) { return; }
  frame = requestAnimationFrame(() => {
    frame = 0;
    renderRows();
  });
}

function renderRows() {
  const scroll = element("scroll");
  const list = element("list");
  const previousScrollTop = scroll.scrollTop;
  const compact = element("compact-layout").checked;
  const inset = compact ? CARD_GAP : 0;
  const innerWidth = Math.max(1, scroll.clientWidth - inset * 2);
  const columns = compact ? Math.max(1, Math.floor((innerWidth + CARD_GAP) / (300 + CARD_GAP))) : 1;
  const rowHeight = compact ? CARD_HEIGHT : ROW_HEIGHT;
  const cardWidth = compact ? (innerWidth - (columns - 1) * CARD_GAP) / columns : innerWidth;
  list.style.height = `${Math.max(0, Math.ceil(filtered.length / columns) * rowHeight + inset * 2 - (compact ? CARD_GAP : 0))}px`;
  element("catalogue").classList.toggle("compact-cards", compact);
  const heading = element("catalogue").querySelector(".list-heading");
  heading.children[0].textContent = compact ? "Sounds · filename / folder / IWD source" : "Sound / folder";
  heading.children[1].hidden = compact;
  // Keep the same part of the catalogue in view when switching layouts or
  // resizing across a column boundary. Selection and playback are untouched.
  if (previousLayout && (previousLayout.columns !== columns || previousLayout.rowHeight !== rowHeight)) {
    const anchor = Math.max(0, Math.floor((previousScrollTop - previousLayout.inset) / previousLayout.rowHeight)) * previousLayout.columns;
    scroll.scrollTop = Math.floor(anchor / columns) * rowHeight;
  }
  previousLayout = { columns, rowHeight, inset };
  const firstRow = Math.max(0, Math.floor((scroll.scrollTop - inset) / rowHeight) - 2);
  const lastRow = Math.ceil((scroll.scrollTop + scroll.clientHeight - inset) / rowHeight) + 2;
  const first = firstRow * columns;
  const last = Math.min(filtered.length, lastRow * columns);
  const visibleIds = new Set(filtered.slice(first, last).map((entry) => entry.id));
  element("empty").hidden = filtered.length > 0;
  for (const [id, row] of rows) {
    if (!visibleIds.has(id)) {
      row.remove();
      rows.delete(id);
    }
  }
  for (let index = first; index < last; index += 1) {
    const entry = filtered[index];
    let row = rows.get(entry.id);
    if (!row) {
      row = document.createElement("div");
      row.className = "sound-row";
      row.tabIndex = 0;
      row.setAttribute("role", "group");
      row.setAttribute("aria-label", entry.name);
      const play = document.createElement("button");
      play.className = "row-play";
      play.setAttribute("aria-label", `Play ${entry.name}`);
      play.title = "Play sound";
      play.addEventListener("click", (event) => {
        event.stopPropagation();
        selectEntry(entry, true);
      });
      const text = document.createElement("div");
      text.className = "row-text";
      const name = document.createElement("span");
      name.className = "row-name";
      name.textContent = entry.name.split("/").at(-1);
      const folder = document.createElement("span");
      folder.className = "row-folder";
      folder.textContent = entry.folder;
      text.append(name, folder);
      const source = document.createElement("span");
      source.className = "row-source";
      source.textContent = entry.sources.length > 1 ? `${entry.sources.length} IWD sources` : entry.sources[0].archive;
      row.append(play, text, source);
      row.addEventListener("click", () => selectEntry(entry, false));
      row.addEventListener("keydown", (event) => {
        if (event.target !== row) { return; }
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          selectEntry(entry, event.key === " ");
        }
      });
      rows.set(entry.id, row);
    }
    const active = selected?.id === entry.id;
    row.style.top = `${inset + Math.floor(index / columns) * rowHeight}px`;
    row.style.left = `${inset + (index % columns) * (cardWidth + (compact ? CARD_GAP : 0))}px`;
    row.style.width = `${cardWidth}px`;
    row.classList.toggle("selected", active);
    row.classList.toggle("failed", failures.has(entry.id));
    row.title = failures.has(entry.id) ? `${entry.name}\n${failures.get(entry.id)}` : entry.name;
    row.children[0].textContent = active && loading ? "…" : active && playing ? "Ⅱ" : "▶";
    row.children[0].disabled = scanning;
    const position = index - first;
    if (list.children[position] !== row) {
      list.insertBefore(row, list.children[position] || null);
    }
  }
}

function filter() {
  const query = element("search").value.trim().toLowerCase();
  const folder = element("folder").value;
  const archive = element("archive").value;
  filtered = entries.filter((entry) => (!query || entry.name.toLowerCase().includes(query))
    && (!folder || entry.name.toLowerCase().startsWith(`${folder}/`))
    && (!archive || entry.sources.some((source) => source.archive === archive)));
  element("count").textContent = `${filtered.length.toLocaleString()} of ${entries.length.toLocaleString()} sounds`;
  element("scroll").scrollTop = 0;
  element("empty").textContent = entries.length ? "No sounds match these filters." : "No WAV sound entries found in main IWD archives.";
  scheduleRows();
}

function updateControls() {
  const ready = Boolean(selection) && !loading && !scanning;
  for (const id of ["play", "stop", "restart", "seek"]) {
    element(id).disabled = !ready;
  }
  element("copy").disabled = !selected;
  element("source").disabled = !selected || loading || scanning;
  element("play").textContent = playing ? "Pause" : "Play";
  element("playing-indicator").textContent = loading ? "Loading…" : playing ? "Playing" : ready ? "Stopped / paused" : selected ? "Preview unavailable" : "No sound selected";
  scheduleRows();
}

function resetPlayer() {
  selected = null;
  selection = null;
  playing = false;
  loading = false;
  element("filename").textContent = "Select a sound";
  element("sound-path").textContent = "Browse the sounds stored in your game’s IWD archives.";
  element("source").replaceChildren();
  for (const id of ["encoding", "channels", "rate", "size"]) { element(id).textContent = "—"; }
  element("seek").value = "0";
  element("elapsed").textContent = "0:00.00";
  element("duration").textContent = "0:00.00";
  updateControls();
}

async function selectEntry(entry, play = false, keepSource = false) {
  if (scanning || !desktop) { return; }
  const request = ++selectionRequest;
  selected = entry;
  selection = null;
  playing = false;
  loading = true;
  if (!keepSource) {
    const archive = element("archive").value;
    sourceIndex = Math.max(0, entry.sources.findIndex((source) => source.archive === archive));
  }
  element("filename").textContent = entry.name.split("/").at(-1);
  element("sound-path").textContent = `sound/${entry.name}`;
  element("source").replaceChildren();
  for (const [index, source] of entry.sources.entries()) {
    option(element("source"), index, `${source.archive} · ${bytes(source.bytes)}`);
  }
  element("source").value = String(sourceIndex);
  for (const id of ["encoding", "channels", "rate"]) { element(id).textContent = "—"; }
  element("size").textContent = bytes(entry.sources[sourceIndex].bytes);
  element("elapsed").textContent = "0:00.00";
  element("duration").textContent = "—";
  element("seek").value = "0";
  updateControls();
  message(`Loading ${entry.name}…`);
  try {
    const result = await invoke("select_sound", { id: entry.id, source: sourceIndex, play });
    if (request !== selectionRequest) { return; }
    selection = result;
    playing = play;
    failures.delete(entry.id);
    element("encoding").textContent = result.info.encoding;
    element("channels").textContent = result.info.channels === 1 ? "Mono" : "Stereo";
    element("rate").textContent = `${result.info.sample_rate.toLocaleString()} Hz`;
    element("size").textContent = bytes(result.info.bytes);
    element("duration").textContent = `${result.info.duration_estimated ? "≈ " : ""}${time(result.info.duration)}`;
    element("duration").title = result.info.duration_estimated ? "Compressed duration includes codec padding." : "";
    element("seek").max = String(result.info.duration);
    message(play ? `Playing ${entry.name}` : "Sound loaded. Press Play to listen.");
  } catch (error) {
    if (request !== selectionRequest) { return; }
    failures.set(entry.id, String(error));
    message(`Could not preview ${entry.name}: ${error}`, true);
  } finally {
    if (request === selectionRequest) {
      loading = false;
      updateControls();
    }
  }
}

async function command(action, value = 0) {
  if (!desktop) { return; }
  const request = selectionRequest;
  try {
    const result = await invoke("transport", { action, value, generation: selection?.generation || 0 });
    if (request === selectionRequest && action < 4) { applyStatus(result); }
    return result;
  } catch (error) {
    if (request === selectionRequest) { message(String(error), true); }
    throw error;
  }
}

function applyStatus(status) {
  if (!selection || status.generation !== selection.generation) { return; }
  const changed = playing !== status.playing;
  playing = status.playing;
  if (!seeking) {
    element("seek").value = String(status.position);
    element("elapsed").textContent = time(status.position);
  }
  if (status.error) {
    message(status.error, true);
    playing = false;
    selection = null;
    updateControls();
    return;
  }
  if (changed) { updateControls(); }
  if (status.ended) { element("playing-indicator").textContent = "Finished"; }
}

async function scan(force = false) {
  if (scanning || !desktop) { return; }
  scanning = true;
  selectionRequest += 1;
  resetPlayer();
  for (const id of ["rescan", "cache", "clear-filters", "folder", "archive", "search"]) { element(id).disabled = true; }
  message("Reading IWD archive directories…");
  const started = performance.now();
  const progress = new Channel();
  progress.onmessage = (update) => {
    message(`Reading IWDs ${update.completed}/${update.total}${update.archive ? ` · ${update.archive}` : ""} · ${update.sounds.toLocaleString()} sound entries`);
  };
  try {
    const index = await invoke("scan", { force, progress });
    entries = index.entries;
    failures.clear();
    rows.clear();
    element("list").replaceChildren();
    element("folder").replaceChildren();
    option(element("folder"), "", "All folders");
    const folders = new Map();
    for (const entry of entries) {
      const parts = entry.name.split("/");
      if (parts.length > 1) {
        const group = parts.slice(0, Math.min(2, parts.length - 1)).join("/");
        folders.set(group.toLowerCase(), group);
      }
    }
    for (const [key, label] of [...folders].sort((a, b) => a[0].localeCompare(b[0]))) { option(element("folder"), key, label); }
    element("archive").replaceChildren();
    option(element("archive"), "", "All archives");
    for (const name of index.archives) { option(element("archive"), name, name); }
    element("search").value = "";
    filter();
    element("statistics").textContent = `${index.entries.length.toLocaleString()} sound paths · ${index.members.toLocaleString()} archive entries · ${index.archives.length} IWD archives. ${index.cached ? "Cached catalogue" : "Archive directories indexed"} in ${((performance.now() - started) / 1000).toFixed(2)} s. Audio is checked when selected. Multiple sources remain selectable.`;
    element("warnings").hidden = index.warnings.length === 0;
    element("warnings").textContent = index.warnings.join("\n");
    message(index.warnings.length ? "Catalogue loaded with warnings. See Catalogue information." : `${entries.length.toLocaleString()} sounds indexed. Select a row or press its play button.`, index.warnings.length > 0);
  } catch (error) {
    message(`Could not read the sound catalogue: ${error}`, true);
    element("empty").textContent = "Could not read IWD archives. Click Rescan to retry.";
  } finally {
    scanning = false;
    for (const id of ["rescan", "cache", "clear-filters", "folder", "archive", "search"]) { element(id).disabled = false; }
    element("status").textContent = element("message").classList.contains("error") ? "Error" : "Ready";
    updateControls();
  }
}

async function showCache() {
  try {
    const info = await invoke("cache_info");
    element("cache-path").value = info.path;
    element("cache-size").textContent = `${bytes(info.bytes)} · catalogue only`;
    element("cache-dialog").showModal();
  } catch (error) { message(String(error), true); }
}

element("scroll").addEventListener("scroll", scheduleRows);
new ResizeObserver(scheduleRows).observe(element("scroll"));
element("compact-layout").addEventListener("change", scheduleRows);
element("search").addEventListener("input", filter);
element("folder").addEventListener("change", filter);
element("archive").addEventListener("change", filter);
element("clear-filters").addEventListener("click", () => {
  element("search").value = "";
  element("folder").value = "";
  element("archive").value = "";
  filter();
});
element("rescan").addEventListener("click", () => scan(true));
element("source").addEventListener("change", () => {
  sourceIndex = Number(element("source").value);
  if (selected) { selectEntry(selected, false, true); }
});
element("play").addEventListener("click", () => command(playing ? 1 : 0).catch(() => {}));
element("stop").addEventListener("click", () => command(2).catch(() => {}));
element("restart").addEventListener("click", async () => {
  try { await command(2); await command(0); } catch { /* The command displayed the error. */ }
});
element("seek").addEventListener("input", () => {
  seeking = true;
  element("elapsed").textContent = time(element("seek").value);
});
element("seek").addEventListener("change", async () => {
  try { await command(3, Number(element("seek").value)); } catch { /* Error already shown. */ }
  seeking = false;
});
element("repeat").addEventListener("change", () => command(5, Number(element("repeat").checked)).catch(() => {}));
function updateVolume() {
  const volume = Number(element("volume").value);
  element("volume-value").textContent = `${muted ? 0 : volume}%`;
  element("mute").textContent = muted ? "Unmute" : "Mute";
  element("mute").setAttribute("aria-label", muted ? "Unmute" : "Mute");
  element("mute").title = muted ? "Unmute" : "Mute";
  command(4, muted ? 0 : volume / 100).catch(() => {});
}
element("volume").addEventListener("input", updateVolume);
element("mute").addEventListener("click", () => { muted = !muted; updateVolume(); });
element("copy").addEventListener("click", async () => {
  if (!selected) { return; }
  try { await navigator.clipboard.writeText(selected.name); message(`Copied ${selected.name}`); }
  catch (error) { message(`Could not copy the sound path: ${error}`, true); }
});
element("cache").addEventListener("click", showCache);
element("clear-cache").addEventListener("click", async () => {
  element("clear-cache").disabled = true;
  try {
    await invoke("clear_cache");
    element("cache-size").textContent = "0 B · catalogue cleared";
    message("Saved catalogue cleared. Click Rescan to rebuild it, or reopen the app.");
  } catch (error) { message(String(error), true); }
  finally { element("clear-cache").disabled = false; }
});
element("about").addEventListener("click", () => {
  element("about-dialog").showModal();
  element("repo-url").focus();
  element("repo-url").select();
});
element("repo-url").addEventListener("click", () => element("repo-url").select());
element("cache-path").addEventListener("click", () => element("cache-path").select());
for (const button of document.querySelectorAll("[data-close]")) {
  button.addEventListener("click", () => element(button.dataset.close).close());
}
element("quit").addEventListener("click", () => invoke("quit"));
setInterval(async () => {
  if (!desktop || !selection || loading || scanning || polling) { return; }
  polling = true;
  const request = selectionRequest;
  try {
    const status = await invoke("playback_status");
    if (request === selectionRequest) { applyStatus(status); }
  } catch (error) { message(String(error), true); }
  finally { polling = false; }
}, 200);

if (desktop) {
  try {
    const startup = await invoke("startup");
    element("version").textContent = `WaW Sound Player | ${startup.version}`;
    element("environment").textContent = startup.development ? "Development" : "Production";
    element("app").hidden = false;
    element("footer").hidden = false;
    await scan();
  } catch (error) {
    element("startup-message").textContent = String(error);
    element("startup-dialog").showModal();
  }
} else {
  element("app").hidden = false;
  element("footer").hidden = false;
  element("environment").textContent = "Browser preview";
  element("empty").textContent = "Open the desktop app to browse your IWD sounds.";
  element("count").textContent = "Desktop app required";
  for (const id of ["rescan", "cache", "volume", "mute", "repeat"]) { element(id).disabled = true; }
  message("UI preview only. Run npm run dev to read and play game sounds.");
}
