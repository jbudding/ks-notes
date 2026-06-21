// Small enhancements; everything works without JS except htmx flows.

function autosize(el) {
  el.style.height = "auto";
  el.style.height = el.scrollHeight + "px";
}

function localizeTimes(root) {
  root.querySelectorAll("time[datetime]").forEach(function (t) {
    var d = new Date(t.getAttribute("datetime"));
    if (!isNaN(d)) {
      t.textContent = d.toLocaleString(undefined, {
        year: "numeric", month: "short", day: "numeric",
        hour: "2-digit", minute: "2-digit",
      });
    }
  });
}

document.addEventListener("input", function (e) {
  if (e.target.matches("textarea")) autosize(e.target);
});

// Ctrl+Enter / Cmd+Enter submits the enclosing composer or editor form.
document.addEventListener("keydown", function (e) {
  if ((e.ctrlKey || e.metaKey) && e.key === "Enter" && e.target.matches("textarea")) {
    var form = e.target.closest("form");
    if (form) {
      e.preventDefault();
      if (window.htmx && form.hasAttribute("hx-post")) htmx.trigger(form, "submit");
      else if (window.htmx && form.hasAttribute("hx-put")) htmx.trigger(form, "submit");
      else form.requestSubmit();
    }
  }
});

// Copy share link buttons: <button data-copy="...">
document.addEventListener("click", function (e) {
  var btn = e.target.closest("[data-copy]");
  if (btn) {
    navigator.clipboard.writeText(new URL(btn.getAttribute("data-copy"), location.href).href);
    var old = btn.textContent;
    btn.textContent = "Copied!";
    setTimeout(function () { btn.textContent = old; }, 1200);
  }
});

document.addEventListener("DOMContentLoaded", function () {
  localizeTimes(document);
});

// Keep the hidden `resources` field in sync with the finished upload chips.
function syncResources(chipsEl) {
  var form = chipsEl.closest("form");
  if (!form) return;
  var hidden = form.querySelector(".resource-uids");
  if (hidden) {
    var uids = [];
    chipsEl.querySelectorAll(".chip[data-uid]").forEach(function (c) {
      uids.push(c.getAttribute("data-uid"));
    });
    hidden.value = uids.join(" ");
  }
}

document.addEventListener("click", function (e) {
  var rm = e.target.closest(".chip-remove");
  if (rm) {
    var chips = rm.closest(".attach-chips");
    rm.closest(".chip").remove();
    if (chips) syncResources(chips);
  }
});

// --- Per-file uploads with a progress bar -----------------------------------
// Each selected file uploads on its own request to /resources, in order, so the
// user sees a progress bar per file. The server returns a chip fragment (with
// data-uid) that replaces the progress row once the file lands.

function csrfToken() {
  try {
    return JSON.parse(document.body.getAttribute("hx-headers") || "{}")["X-CSRF-Token"] || "";
  } catch (_) {
    return "";
  }
}

// Track in-flight uploads per form so Save stays disabled until the queue drains.
function setUploading(form, delta) {
  var n = (parseInt(form.dataset.uploading || "0", 10) || 0) + delta;
  if (n < 0) n = 0;
  form.dataset.uploading = n;
  var save = form.querySelector("button[type=submit]");
  if (save) save.disabled = n > 0;
}

function uploadOne(form, chipsEl, file) {
  return new Promise(function (resolve) {
    var row = document.createElement("span");
    row.className = "chip uploading";
    row.innerHTML =
      '<span class="up-name"></span><span class="up-bar"><i class="up-fill"></i></span>';
    row.querySelector(".up-name").textContent = "📎 " + file.name;
    chipsEl.appendChild(row);
    var fill = row.querySelector(".up-fill");

    var xhr = new XMLHttpRequest();
    xhr.open("POST", "/resources");
    xhr.setRequestHeader("X-CSRF-Token", csrfToken());
    xhr.upload.addEventListener("progress", function (e) {
      if (e.lengthComputable) fill.style.width = Math.round((e.loaded / e.total) * 100) + "%";
    });
    xhr.addEventListener("load", function () {
      if (xhr.status >= 200 && xhr.status < 300) {
        row.outerHTML = xhr.responseText; // becomes a real .chip[data-uid]
        syncResources(chipsEl);
      } else {
        row.classList.remove("uploading");
        row.classList.add("failed");
        row.innerHTML =
          '<span class="up-name"></span><button type="button" class="chip-remove" title="Remove">×</button>';
        row.querySelector(".up-name").textContent =
          "⚠ " + file.name + " — " + (xhr.responseText || "HTTP " + xhr.status);
      }
      resolve();
    });
    xhr.addEventListener("error", function () {
      row.classList.remove("uploading");
      row.classList.add("failed");
      var bar = row.querySelector(".up-bar");
      if (bar) bar.remove();
      row.querySelector(".up-name").textContent = "⚠ " + file.name + " — upload failed";
      resolve();
    });

    var fd = new FormData();
    fd.append("file", file);
    xhr.send(fd);
  });
}

document.addEventListener("change", function (e) {
  var input = e.target.closest(".file-input");
  if (!input || !input.files || !input.files.length) return;
  var form = input.closest("form");
  if (!form) return;
  var chipsEl = form.querySelector(".attach-chips");
  if (!chipsEl) return;

  var files = Array.prototype.slice.call(input.files);
  input.value = ""; // allow re-selecting the same file later

  setUploading(form, files.length);
  // Upload sequentially so progress bars fill one at a time.
  files.reduce(function (chain, file) {
    return chain.then(function () {
      return uploadOne(form, chipsEl, file).then(function () {
        setUploading(form, -1);
      });
    });
  }, Promise.resolve());
});

// Re-localize timestamps and sync upload chips inside htmx-swapped fragments.
document.body.addEventListener("htmx:afterSwap", function (e) {
  localizeTimes(e.target);
  if (e.target.classList && e.target.classList.contains("attach-chips")) {
    syncResources(e.target);
  }
});

// Give the Save button immediate feedback while the form request is in flight:
// turn it orange and swap the label to "Saving…". Scoped to form submits, so
// pin/archive/edit/delete buttons (their own elt, no submit child) are untouched.
function saveButton(detail) {
  var form = detail && detail.elt;
  if (!form || form.tagName !== "FORM") return null;
  return form.querySelector("button[type=submit]");
}

document.body.addEventListener("htmx:beforeRequest", function (e) {
  var btn = saveButton(e.detail);
  if (!btn) return;
  btn.dataset.label = btn.textContent;
  btn.textContent = "Saving…";
  btn.classList.add("saving");
});

document.body.addEventListener("htmx:afterRequest", function (e) {
  var btn = saveButton(e.detail);
  // On success the form is swapped away; this restores the button on errors or
  // on the composer (which resets in place rather than being replaced).
  if (!btn || btn.dataset.label === undefined) return;
  btn.textContent = btn.dataset.label;
  delete btn.dataset.label;
  btn.classList.remove("saving");
});
