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

document.addEventListener("change", function (e) {
  var input = e.target.closest(".file-input");
  if (!input || !input.files || !input.files.length) return;
  var form = input.closest("form");
  if (!form) return;
  var ta = form.querySelector("textarea[name=content]");
  if (!ta) return;

  var files = Array.prototype.slice.call(input.files);
  input.value = ""; // allow re-selecting the same file later

  setUploading(form, files.length);
  // Upload sequentially so the inline tokens land in selection order.
  files.reduce(function (chain, file, idx) {
    return chain.then(function () {
      return uploadOne(ta, file, Date.now() + "-" + idx).then(function () {
        setUploading(form, -1);
      });
    });
  }, Promise.resolve());
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

document.body.addEventListener("htmx:afterSwap", function (e) {
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

  // After a note is posted the composer resets to empty; re-seed it for the
  // next note (deferred so the inline reset handler runs first).
  var form = e.detail && e.detail.elt;
  if (form && form.classList && form.classList.contains("composer") && e.detail.successful) {
    setTimeout(function () {
      seedSelfTag(form.querySelector("textarea[data-self-tag]"));
    }, 0);
  }
});

// Seed an empty composer with the author's own #tag on its own line, a blank
// line below the cursor, so every note is self-tagged but typed above it.
function seedSelfTag(ta) {
  if (!ta || ta.value.trim() !== "") return;
  var tag = ta.getAttribute("data-self-tag");
  if (!tag) return;
  ta.value = "\n\n#" + tag;
  try {
    ta.setSelectionRange(0, 0);
  } catch (_) {}
}

document.querySelectorAll("textarea[data-self-tag]").forEach(seedSelfTag);
