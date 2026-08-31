const $ = (id) => document.getElementById(id);
let scrubbing = false;
let adjusting = false;
let paused = true;
let muted = false;

const send = (method, path, body) =>
  fetch(path, {
    method,
    headers: body ? { "content-type": "application/json" } : {},
    body: body ? JSON.stringify(body) : undefined,
  });

const clock = (seconds) => {
  const whole = Math.floor(seconds);
  const minutes = Math.floor(whole / 60);
  return `${minutes}:${String(whole % 60).padStart(2, "0")}`;
};

$("add").addEventListener("submit", async (event) => {
  event.preventDefault();
  const field = event.target.url;
  await send("POST", "/queue", { url: field.value });
  field.value = "";
  refresh();
});

$("clear").addEventListener("click", async () => {
  await send("DELETE", "/queue");
  refresh();
});

$("playpause").addEventListener("click", async () => {
  await send("POST", paused ? "/playback/play" : "/playback/pause");
  refresh();
});

for (const button of document.querySelectorAll("[data-post]")) {
  button.addEventListener("click", async () => {
    const delta = button.dataset.delta;
    await (delta
      ? send("PATCH", button.dataset.post, { delta: Number(delta) })
      : send("POST", button.dataset.post));
    refresh();
  });
}

$("mute").addEventListener("click", async () => {
  await send("PATCH", "/status", { muted: !muted });
  refresh();
});

$("volume").addEventListener("input", () => (adjusting = true));
$("volume").addEventListener("change", async () => {
  adjusting = false;
  await send("PATCH", "/status", { volume: Number($("volume").value) });
  refresh();
});

$("scrubber").addEventListener("input", () => (scrubbing = true));
$("scrubber").addEventListener("change", async () => {
  scrubbing = false;
  await send("PUT", "/playback/position", {
    seconds: Number($("scrubber").value),
  });
  refresh();
});

async function refresh() {
  const [status, queue] = await Promise.all([
    fetch("/status").then((r) => r.json()),
    fetch("/queue").then((r) => r.json()),
  ]);

  const playing = status.playback;
  paused = !playing || playing.paused;
  $("playpause").textContent = paused ? "⏵" : "⏸";
  $("playpause").title = paused ? "Play" : "Pause";
  $("playpause").ariaLabel = $("playpause").title;
  $("title").textContent = playing
    ? (playing.current.title ?? playing.current.url)
    : "Nothing playing";

  const position = playing?.position ?? 0;
  const duration = playing?.duration ?? 0;
  $("scrubber").max = duration;
  $("scrubber").disabled = !duration;
  if (!scrubbing) $("scrubber").value = position;
  $("clock").textContent = duration
    ? `${clock(position)} / ${clock(duration)}`
    : "";

  muted = status.muted;
  $("mute").textContent = muted ? "Unmute" : "Mute";
  $("mute").title = $("mute").textContent;
  $("mute").ariaLabel = $("mute").textContent;
  if (!adjusting) $("volume").value = status.volume;

  $("errors").textContent = status.errors.map((error) => error.message).join("\n");

  $("queue").replaceChildren(
    ...queue.map((track, index) => {
      const item = document.createElement("li");
      item.textContent = track.title ?? track.url;
      if (index === playing?.current_index) item.className = "current";
      return item;
    }),
  );
}

refresh();
setInterval(refresh, 1000);
