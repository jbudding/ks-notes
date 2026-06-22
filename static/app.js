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

// Full-page toggle: maximize a single note, click again (or Esc) to restore.
function setExpanded(card, on) {
  card.classList.toggle("memo-expanded", on);
  document.body.classList.toggle("has-expanded-memo", on);
  var btn = card.querySelector(".memo-expand");
  if (btn) {
    btn.textContent = on ? "Exit" : "Full";
    btn.title = on ? "Exit full page" : "Full page";
  }
}

document.addEventListener("click", function (e) {
  var btn = e.target.closest(".memo-expand");
  if (!btn) return;
  var card = btn.closest(".memo-card");
  if (card) setExpanded(card, !card.classList.contains("memo-expanded"));
});

document.addEventListener("keydown", function (e) {
  if (e.key !== "Escape") return;
  var card = document.querySelector(".memo-card.memo-expanded");
  if (card) setExpanded(card, false);
});

// --- Segmented import --------------------------------------------------------
// The browser streams the file up to /import/stream (so a large export is never
// loaded into memory here), and the server streams back one NDJSON line per note
// — {i, total, uuid, status} — which we render as live "n of X" progress plus a
// per-note added/merged/skipped log. Falls back to the plain multipart /import
// (full page reload) when streaming responses aren't available.
function importSegmented(form, file) {
  var owEl = form.querySelector('input[name=overwrite]');
  var overwrite = !!(owEl && owEl.checked);
  var csrfEl = form.querySelector('input[name=csrf_token]');
  var fill = document.getElementById("import-fill");
  var status = document.getElementById("import-status");
  var log = document.getElementById("import-log");
  var btn = form.querySelector("button[type=submit]");

  document.getElementById("import-progress").hidden = false;
  log.innerHTML = "";
  fill.style.width = "0%";
  status.textContent = "Uploading…";
  if (btn) btn.disabled = true;

  var counts = { added: 0, merged: 0, skipped: 0, error: 0 };
  function finish() {
    status.textContent =
      "Done — " + counts.added + " added, " + counts.merged + " merged, " + counts.skipped + " skipped" +
      (counts.error ? ", " + counts.error + " failed" : "") + ".";
    if (btn) btn.disabled = false;
  }
  function handleLine(line) {
    line = line.trim();
    if (!line) return;
    var o;
    try { o = JSON.parse(line); } catch (_) { return; }
    if (counts[o.status] !== undefined) counts[o.status]++;
    if (o.total) {
      fill.style.width = Math.round((o.i / o.total) * 100) + "%";
      status.textContent = o.i + " of " + o.total + " notes processed";
    }
    var li = document.createElement("li");
    li.className = "import-" + o.status;
    li.innerHTML = '<span class="import-uuid"></span><span class="import-stat"></span>';
    li.querySelector(".import-uuid").textContent = o.uuid;
    li.querySelector(".import-stat").textContent = o.status;
    log.appendChild(li);
  }

  var fd = new FormData();
  fd.append("csrf_token", csrfEl ? csrfEl.value : "");
  if (overwrite) fd.append("overwrite", "true");
  fd.append("file", file); // streamed from disk, not buffered into a JS string

  fetch("/import/stream", { method: "POST", body: fd })
    .then(function (resp) {
      if (!resp.ok || !resp.body || !resp.body.getReader) {
        return resp.text().then(function () {
          alert("Import failed (" + resp.status + ").");
          if (btn) btn.disabled = false;
        });
      }
      var reader = resp.body.getReader();
      var dec = new TextDecoder();
      var buf = "";
      function pump() {
        return reader.read().then(function (res) {
          if (res.done) {
            handleLine(buf);
            finish();
            return;
          }
          buf += dec.decode(res.value, { stream: true });
          var parts = buf.split("\n");
          buf = parts.pop();
          parts.forEach(handleLine);
          return pump();
        });
      }
      return pump();
    })
    .catch(function () {
      alert("Import failed.");
      if (btn) btn.disabled = false;
    });
}

document.addEventListener("submit", function (e) {
  var form = e.target;
  // Need fetch + streaming response support; otherwise let the form post to
  // /import for the plain bulk fallback.
  if (!form || form.id !== "import-form" || !window.fetch || !window.ReadableStream) return;
  var input = form.querySelector('input[type=file]');
  var file = input && input.files && input.files[0];
  if (!file) return; // let the required attribute handle the empty case
  e.preventDefault();
  importSegmented(form, file);
});

// --- Inline attachments ------------------------------------------------------
// Selecting files uploads each to /resources; on success a {{attach:UID}} token
// is placed in the note at the cursor (the "attachment point"), so attachments
// live inline with text above and below. An in-flight placeholder token shows
// progress and is swapped for the real one when the upload lands.

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

// Replace `find` with `replacement` in the textarea, keeping it tidy.
function replaceInTextarea(ta, find, replacement) {
  var i = ta.value.indexOf(find);
  if (i < 0) return;
  ta.value = ta.value.slice(0, i) + replacement + ta.value.slice(i + find.length);
}

// Place `text` at the cursor, or into the first empty `{{attach}}` placeholder
// if one exists. Surrounds the token with blank lines so it renders as a block.
function placeAttachment(ta, text) {
  var block = "\n\n" + text + "\n\n";
  if (ta.value.indexOf("{{attach}}") >= 0) {
    replaceInTextarea(ta, "{{attach}}", text);
    return;
  }
  var start = ta.selectionStart || 0;
  var end = ta.selectionEnd || start;
  ta.value = ta.value.slice(0, start) + block + ta.value.slice(end);
  var pos = start + block.length;
  try {
    ta.focus();
    ta.setSelectionRange(pos, pos);
  } catch (_) {}
}

function uploadOne(ta, file, nonce) {
  // A unique placeholder token marks where this upload will land.
  var placeholder = "{{attach:uploading-" + nonce + "}}";
  placeAttachment(ta, "⏳ " + file.name + " " + placeholder);

  return new Promise(function (resolve) {
    var marker = "⏳ " + file.name + " " + placeholder;
    var xhr = new XMLHttpRequest();
    xhr.open("POST", "/resources");
    xhr.setRequestHeader("X-CSRF-Token", csrfToken());
    xhr.addEventListener("load", function () {
      if (xhr.status >= 200 && xhr.status < 300) {
        var m = xhr.responseText.match(/data-uid="([^"]+)"/);
        var uid = m ? m[1] : "";
        replaceInTextarea(ta, marker, uid ? "{{attach:" + uid + "}}" : "");
      } else {
        replaceInTextarea(ta, marker, "");
        alert("Upload failed for " + file.name + ": " + (xhr.responseText || "HTTP " + xhr.status));
      }
      resolve();
    });
    xhr.addEventListener("error", function () {
      replaceInTextarea(ta, marker, "");
      alert("Upload failed for " + file.name);
      resolve();
    });

    var fd = new FormData();
    fd.append("file", file);
    xhr.send(fd);
  });
}

// Upload a list of files into a form's note, inserting an attachment token per
// file. Sequential so the inline tokens land in order.
function uploadFiles(form, ta, files) {
  if (!files.length) return;
  setUploading(form, files.length);
  files.reduce(function (chain, file, idx) {
    return chain.then(function () {
      return uploadOne(ta, file, Date.now() + "-" + idx).then(function () {
        setUploading(form, -1);
      });
    });
  }, Promise.resolve());
}

document.addEventListener("change", function (e) {
  var input = e.target.closest(".file-input");
  if (!input || !input.files || !input.files.length) return;
  var form = input.closest("form");
  if (!form) return;
  var ta = form.querySelector("textarea[name=content]");
  if (!ta) return;

  var files = Array.prototype.slice.call(input.files);
  input.value = ""; // allow re-selecting the same file later
  uploadFiles(form, ta, files);
});

// Wrap a clipboard image blob as a named File so it uploads like a normal attach.
function imageFile(blob, type) {
  var ext = (type || "image/png").split("/")[1] || "png";
  return new File([blob], "pasted-" + Date.now() + "." + ext, { type: type || blob.type });
}

// Paste-image icon: read an image off the clipboard and insert it at the cursor.
document.addEventListener("click", function (e) {
  var btn = e.target.closest(".paste-image");
  if (!btn) return;
  var form = btn.closest("form");
  var ta = form && form.querySelector("textarea[name=content]");
  if (!ta) return;
  if (!navigator.clipboard || !navigator.clipboard.read) {
    alert("Pasting from the clipboard needs a secure (https) page. You can still press Ctrl+V in the note.");
    return;
  }
  navigator.clipboard
    .read()
    .then(function (items) {
      var jobs = [];
      items.forEach(function (item) {
        item.types.forEach(function (type) {
          if (type.indexOf("image/") === 0) {
            jobs.push(item.getType(type).then(function (b) { return imageFile(b, type); }));
          }
        });
      });
      if (!jobs.length) {
        alert("No image found on the clipboard.");
        return;
      }
      Promise.all(jobs).then(function (files) { uploadFiles(form, ta, files); });
    })
    .catch(function (err) {
      alert("Couldn't read the clipboard: " + (err && err.message ? err.message : err));
    });
});

// Ctrl+V of an image into the note also attaches it inline (works without https).
document.addEventListener("paste", function (e) {
  var ta = e.target;
  if (!ta || ta.tagName !== "TEXTAREA" || ta.name !== "content" || !e.clipboardData) return;
  var files = [];
  Array.prototype.forEach.call(e.clipboardData.items || [], function (it) {
    if (it.kind === "file" && it.type.indexOf("image/") === 0) {
      var blob = it.getAsFile();
      if (blob) files.push(imageFile(blob, it.type));
    }
  });
  if (!files.length) return; // let normal text paste through
  e.preventDefault();
  var form = ta.closest("form");
  if (form) uploadFiles(form, ta, files);
});

// After any swap, reconcile full-page state. We scan the DOM rather than trust
// e.target because an outerHTML swap fires afterSwap on the parent, not the new
// node. Editing always opens full screen; once nothing is expanded, unlock body.
function reconcileExpanded() {
  var editor = document.querySelector(".memo-editor");
  if (editor) {
    setExpanded(editor, true);
    var ta = editor.querySelector("textarea");
    if (ta) autosize(ta);
  }
  if (!document.querySelector(".memo-card.memo-expanded")) {
    document.body.classList.remove("has-expanded-memo");
  }
}

// Use afterSettle, not afterSwap: with htmx's settle transition the swapped-in
// editor isn't matchable as `.memo-editor` until settle completes, so reconciling
// on afterSwap would miss it and editing wouldn't open full screen.
document.body.addEventListener("htmx:afterSettle", function (e) {
  localizeTimes(e.target);
  reconcileExpanded();
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
  if (btn && btn.dataset.label !== undefined) {
    btn.textContent = btn.dataset.label;
    delete btn.dataset.label;
    btn.classList.remove("saving");
  }
});
