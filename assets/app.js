const $ = (id) => document.getElementById(id);
let scrubbing = false;
let adjusting = false;
let paused = true;
let muted = false;
let renderedQueue = null;

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

$("add-link").addEventListener("click", () => $("add-dialog").showModal());
$("add-cancel").addEventListener("click", () => $("add-dialog").close());
// Covers Esc and every button, so a dismissed link never reappears.
$("add-dialog").addEventListener("close", () => $("add").reset());

$("add").addEventListener("submit", async (event) => {
  event.preventDefault();
  await send("POST", "/queue", { url: event.target.url.value });
  $("add-dialog").close();
});

$("clear").addEventListener("click", async () => {
  await send("DELETE", "/queue");
});

$("playpause").addEventListener("click", async () => {
  await send("POST", paused ? "/playback/play" : "/playback/pause");
});

for (const button of document.querySelectorAll("[data-post]")) {
  button.addEventListener("click", async () => {
    const delta = button.dataset.delta;
    await (delta
      ? send("PATCH", button.dataset.post, { delta: Number(delta) })
      : send("POST", button.dataset.post));
  });
}

$("mute").addEventListener("click", async () => {
  await send("PATCH", "/status", { muted: !muted });
});

$("volume").addEventListener("input", () => (adjusting = true));
$("volume").addEventListener("change", async () => {
  adjusting = false;
  await send("PATCH", "/status", { volume: Number($("volume").value) });
});

$("quality").addEventListener("change", async () => {
  await send("PATCH", "/status", { max_height: Number($("quality").value) });
});

$("scrubber").addEventListener("input", () => (scrubbing = true));
$("scrubber").addEventListener("change", async () => {
  scrubbing = false;
  await send("PUT", "/playback/position", {
    seconds: Number($("scrubber").value),
  });
});

function render({ status, queue }) {
  const playing = status.playback;
  paused = !playing || playing.paused;
  $("playpause")
    .querySelector("use")
    .setAttribute("href", paused ? "#icon-play" : "#icon-pause");
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

  // A cap with no matching option selects nothing, so show it as Custom.
  $("quality").value = status.max_height ?? "";
  if ($("quality").selectedIndex === -1) $("quality").value = "";

  $("errors").textContent = status.errors.map((error) => error.message).join("\n");

  const queueState = JSON.stringify([queue, playing?.current_index]);
  if (queueState !== renderedQueue) {
    renderedQueue = queueState;
    renderQueue(queue, playing?.current_index);
  }
}

function renderQueue(queue, currentIndex) {
  $("queue").replaceChildren(
    ...queue.map((track, index) => {
      const button = document.createElement("button");
      button.textContent = track.title ?? track.url;
      button.addEventListener("click", async () => {
        await send("PUT", "/playback/current_index", { index });
      });
      const item = document.createElement("li");
      item.append(button);
      if (index === currentIndex) item.className = "current";
      return item;
    }),
  );
}

// The stream opens with the current state and sends every change after it,
// and EventSource reconnects on its own if the server goes away.
new EventSource("/events").onmessage = (event) =>
  render(JSON.parse(event.data));
