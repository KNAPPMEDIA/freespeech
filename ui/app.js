"use strict";

/* FreeSpeech: Hauptfenster.
   Spricht mit dem Rust-Kern über window.__TAURI__ (invoke / listen).
   Ohne Tauri (Vorschau im Browser) übernimmt ein eingebauter Mock. */

(function () {
  /* ------------------------------------------------------------------ */
  /* Verbindung zum Kern                                                 */
  /* ------------------------------------------------------------------ */

  let invoke;
  let listen;
  if (window.__TAURI__) {
    ({ invoke } = window.__TAURI__.core);
    ({ listen } = window.__TAURI__.event);
  } else {
    ({ invoke, listen } = erzeugeMock());
  }

  /* ------------------------------------------------------------------ */
  /* Zustand                                                             */
  /* ------------------------------------------------------------------ */

  let snap = null;                                   // letzter Snapshot vom Kern
  let status = { state: "idle", message: "" };       // letzter Status (Event oder Snapshot)
  let seite = "diktat";
  let lauschZiel = null;                             // "hotkey" | "cancel_key" | null
  let leerenOffen = false;
  let herunterladen = null;                          // laufender Download oder letzter Fehler: { id, done_mb, total_mb, state, message }
  let ladeNr = 0;
  let speicherKette = Promise.resolve();
  const entprellt = new Map();
  const signatur = {};                               // merkt, was zuletzt gezeichnet wurde

  const ENTPRELLEN_MS = 500;
  const SPRACHEN = ["auto", "de", "en", "fr", "es", "it", "nl", "pt", "pl", "tr", "ru", "uk", "ja"];

  /* ------------------------------------------------------------------ */
  /* Kleine Helfer                                                       */
  /* ------------------------------------------------------------------ */

  const $ = (id) => document.getElementById(id);

  function el(tag, attrs, ...kinder) {
    const n = document.createElement(tag);
    if (attrs) {
      for (const [k, v] of Object.entries(attrs)) {
        if (v == null || v === false) continue;
        if (k === "class") n.className = v;
        else if (k.startsWith("on")) n.addEventListener(k.slice(2), v);
        else n.setAttribute(k, v === true ? "" : String(v));
      }
    }
    for (const kind of kinder.flat()) {
      if (kind != null) n.append(kind);              // Strings werden Textknoten
    }
    return n;
  }

  // Feste, eigene Pfaddaten. Kein Nutzertext.
  const SYMBOLE = {
    kopieren: '<rect x="5.5" y="5.5" width="8" height="8" rx="1.5"/><path d="M10.5 5.5v-2a1 1 0 0 0-1-1h-6a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2"/>',
    loeschen: '<path d="M2.8 4.5h10.4"/><path d="M6.3 4.5V3h3.4v1.5"/><path d="M4.2 4.5l.6 8.5h6.4l.6-8.5"/>',
    kreuz: '<path d="M4 4l8 8M12 4l-8 8"/>',
    pfeil: '<path d="M2.5 8h10"/><path d="M9 4.5 12.5 8 9 11.5"/>',
    laden: '<path d="M8 2.5v7.4"/><path d="M4.8 6.8 8 10 11.2 6.8"/><path d="M3 13h10"/>',
    haken: '<path d="M3.4 8.6 6.5 11.6 12.6 4.8"/>',
  };
  const SVG_NS = "http://www.w3.org/2000/svg";

  function symbol(name) {
    const svg = document.createElementNS(SVG_NS, "svg");
    svg.setAttribute("class", "symbol");
    svg.setAttribute("viewBox", "0 0 16 16");
    svg.setAttribute("width", "16");
    svg.setAttribute("height", "16");
    svg.setAttribute("aria-hidden", "true");
    svg.innerHTML = SYMBOLE[name];
    return svg;
  }

  function fehlertext(err) {
    if (typeof err === "string") return err;
    if (err && typeof err.message === "string") return err.message;
    return String(err);
  }

  function gleicherPfad(a, b) {
    if (!a || !b) return false;
    const norm = (p) => String(p).replace(/\//g, "\\").toLowerCase();
    return norm(a) === norm(b);
  }

  function entprelle(schluessel, fn) {
    clearTimeout(entprellt.get(schluessel));
    entprellt.set(schluessel, setTimeout(() => {
      entprellt.delete(schluessel);
      fn();
    }, ENTPRELLEN_MS));
  }

  function entprellenAbbrechen(schluessel) {
    clearTimeout(entprellt.get(schluessel));
    entprellt.delete(schluessel);
  }

  /* ---------- Zahlen und Zeiten, deutsch ---------- */

  const zahl0 = new Intl.NumberFormat("de-DE", { maximumFractionDigits: 0 });
  const zahl1 = new Intl.NumberFormat("de-DE", { minimumFractionDigits: 1, maximumFractionDigits: 1 });
  const fmtUhr = new Intl.DateTimeFormat("de-DE", { hour: "2-digit", minute: "2-digit" });
  const fmtTag = new Intl.DateTimeFormat("de-DE", { day: "2-digit", month: "2-digit", year: "numeric" });

  function fmtLadezeit(ms) {
    if (!(ms >= 0)) return "unbekannt";
    return ms < 1000 ? `${zahl0.format(ms)} ms` : `${zahl1.format(ms / 1000)} s`;
  }

  function fmtDauer(ms) {
    if (!(ms >= 0)) return "unbekannt";
    const s = ms / 1000;
    if (s < 60) return `${zahl1.format(s)} s`;
    const ganz = Math.round(s);
    return `${Math.floor(ganz / 60)}:${String(ganz % 60).padStart(2, "0")} min`;
  }

  function tempoVon(e) {
    return e && e.audio_ms > 0 && e.took_ms > 0 ? e.audio_ms / e.took_ms : null;
  }

  function fmtTempo(x) {
    return `${zahl1.format(x)}×`;
  }

  function tagesSchluessel(d) {
    return `${d.getFullYear()}-${d.getMonth()}-${d.getDate()}`;
  }

  function fmtZeitpunkt(id) {
    const d = new Date(Number(id));
    if (Number.isNaN(d.getTime())) return "unbekannt";
    const heute = new Date();
    const gestern = new Date(heute.getFullYear(), heute.getMonth(), heute.getDate() - 1);
    const uhr = fmtUhr.format(d);
    if (tagesSchluessel(d) === tagesSchluessel(heute)) return `Heute, ${uhr}`;
    if (tagesSchluessel(d) === tagesSchluessel(gestern)) return `Gestern, ${uhr}`;
    return `${fmtTag.format(d)}, ${uhr}`;
  }

  /* ---------- Meldungen ---------- */

  let meldungTimer = 0;
  function melde(text) {
    $("meldung-text").textContent = text;
    $("meldung").hidden = false;
    clearTimeout(meldungTimer);
    meldungTimer = setTimeout(() => { $("meldung").hidden = true; }, 9000);
  }

  let gespeichertTimer = 0;
  function zeigeGespeichert() {
    const marken = document.querySelectorAll("[data-gespeichert]");
    marken.forEach((m) => m.classList.add("sichtbar"));
    clearTimeout(gespeichertTimer);
    gespeichertTimer = setTimeout(() => marken.forEach((m) => m.classList.remove("sichtbar")), 1400);
  }

  /* Baut einen Bereich neu und hält dabei den Tastaturfokus an derselben Stelle. */
  function neuAufbauen(behaelter, kinder) {
    const aktiv = document.activeElement;
    const hatteFokus = aktiv && behaelter.contains(aktiv);
    const marke = hatteFokus ? aktiv.getAttribute("data-fokus") : null;
    behaelter.replaceChildren(...kinder);
    if (!hatteFokus) return;
    let ziel = null;
    if (marke) {
      ziel = [...behaelter.querySelectorAll("[data-fokus]")].find((n) => n.getAttribute("data-fokus") === marke);
    }
    (ziel || behaelter.querySelector("button:not(:disabled)") || behaelter).focus({ preventScroll: true });
  }

  /* ------------------------------------------------------------------ */
  /* Pegelstreifen                                                       */
  /* ------------------------------------------------------------------ */

  function erzeugePegel(wurzel, wertEl, anzahl) {
    const ROT_AB = Math.round(anzahl * 0.85);        // die obersten rund 15 % sind rot
    const segmente = [];
    for (let i = 0; i < anzahl; i++) {
      const s = document.createElement("span");
      s.className = i >= ROT_AB ? "seg rot" : "seg";
      s.setAttribute("aria-hidden", "true");
      wurzel.append(s);
      segmente.push(s);
    }

    const ruhig = window.matchMedia("(prefers-reduced-motion: reduce)");
    let ziel = 0;            // Zielwert auf der Anzeige, 0..1
    let anzeige = 0;         // aktuell gezeichneter Wert
    let spitze = 0;          // Spitzenhalte-Wert
    let spitzeSeit = 0;
    let letzterEingang = 0;
    let letzterTakt = 0;
    let rahmen = 0;
    let anAlt = 0;
    let spitzeAlt = -1;
    let wertSeit = 0;

    // Der Kern liefert 0..1 (lineare Amplitude). Die Wurzel spreizt leise Pegel,
    // die Skala darunter ist danach beschriftet (Position p entspricht 40·log10(p) dB).
    const kurve = (v) => Math.sqrt(Math.min(1, Math.max(0, v)));

    function zeichne(jetzt) {
      const an = Math.min(anzahl, Math.round(anzeige * anzahl));
      if (an !== anAlt) {
        const von = Math.min(an, anAlt);
        const bis = Math.max(an, anAlt);
        for (let i = von; i < bis; i++) segmente[i].classList.toggle("an", i < an);
        anAlt = an;
      }
      const sp = spitze > 0.004 ? Math.min(anzahl - 1, Math.max(0, Math.ceil(spitze * anzahl) - 1)) : -1;
      if (sp !== spitzeAlt) {
        if (spitzeAlt >= 0) segmente[spitzeAlt].classList.remove("spitze");
        if (sp >= 0) segmente[sp].classList.add("spitze");
        spitzeAlt = sp;
      }
      if (jetzt - wertSeit > 110) {
        wertSeit = jetzt;
        let text = "−∞ dB";
        if (spitze > 0.02) {
          const db = Math.min(0, 40 * Math.log10(spitze));
          text = `${db < -0.05 ? "−" : ""}${zahl1.format(Math.abs(db))} dB`;
        }
        if (wertEl.textContent !== text) wertEl.textContent = text;
      }
    }

    function takt(jetzt) {
      const dt = Math.min(0.1, Math.max(0, (jetzt - letzterTakt) / 1000));
      letzterTakt = jetzt;
      if (jetzt - letzterEingang > 180) ziel = 0;    // Aufnahme vorbei, kein Signal mehr

      if (ruhig.matches) {
        anzeige = ziel;
      } else if (ziel > anzeige) {
        anzeige += (ziel - anzeige) * Math.min(1, dt * 45);     // schneller Anstieg
      } else {
        anzeige = Math.max(ziel, anzeige - dt * 1.25);          // weiches Abfallen
      }

      if (anzeige >= spitze) {
        spitze = anzeige;
        spitzeSeit = jetzt;
      } else if (jetzt - spitzeSeit > 1100) {
        spitze = ruhig.matches ? anzeige : Math.max(anzeige, spitze - dt * 0.6);
      }

      if (anzeige < 0.002 && spitze < 0.006 && ziel === 0) {
        anzeige = 0;
        spitze = 0;
        wertSeit = 0;
        zeichne(jetzt);
        rahmen = 0;
        return;
      }
      zeichne(jetzt);
      rahmen = requestAnimationFrame(takt);
    }

    return {
      setze(v) {
        ziel = kurve(Number(v) || 0);
        letzterEingang = performance.now();
        if (!rahmen) {
          letzterTakt = letzterEingang;
          rahmen = requestAnimationFrame(takt);
        }
      },
    };
  }

  const pegel = erzeugePegel($("pegel"), $("pegel-wert"), 48);

  /* ------------------------------------------------------------------ */
  /* Bausteine                                                           */
  /* ------------------------------------------------------------------ */

  function zeichneKappen(behaelter, labels) {
    const liste = Array.isArray(labels) && labels.length ? labels : ["nicht belegt"];
    const sig = liste.join("\u0001");
    if (behaelter.dataset.sig === sig) return;
    behaelter.dataset.sig = sig;
    const kinder = [];
    liste.forEach((l, i) => {
      if (i > 0) kinder.push(el("span", { class: "plus", "aria-hidden": "true" }, "+"));
      kinder.push(el("kbd", { class: "kappe" }, String(l)));
    });
    behaelter.replaceChildren(...kinder);
    behaelter.setAttribute("aria-label", liste.join(" plus "));
  }

  function kopierKnopf(text, klasse, fokusMarke) {
    const schrift = el("span", null, "Kopieren");
    const knopf = el("button", { type: "button", class: klasse, "data-fokus": fokusMarke }, schrift);
    let timer = 0;
    knopf.addEventListener("click", async () => {
      try {
        await invoke("copy_text", { text });
        schrift.textContent = "Kopiert";
        knopf.classList.add("bestaetigt");
        clearTimeout(timer);
        timer = setTimeout(() => {
          schrift.textContent = "Kopieren";
          knopf.classList.remove("bestaetigt");
        }, 1500);
      } catch (err) {
        melde(`Kopieren hat nicht geklappt: ${fehlertext(err)}`);
      }
    });
    return knopf;
  }

  function datenpaar(name, wert) {
    return el("span", null,
      el("span", { class: "beschriftung" }, name),
      el("span", { class: "messwert" }, wert));
  }

  /* ------------------------------------------------------------------ */
  /* Status                                                              */
  /* ------------------------------------------------------------------ */

  function statusAnzeige() {
    switch (status.state) {
      case "error": return { zustand: "error", text: status.message || "Fehler" };
      case "loading": return { zustand: "loading", text: "Lädt Modell …" };
      case "recording": return { zustand: "recording", text: "Aufnahme läuft" };
      case "transcribing": return { zustand: "transcribing", text: "Schreibt …" };
      default:
        if (snap && !snap.engine) {
          if (herunterladen && herunterladen.state === "running") {
            return { zustand: "loading", text: "Lädt Modell herunter …" };
          }
          return { zustand: "leer", text: "Kein Modell geladen" };
        }
        return { zustand: "idle", text: "Bereit" };
    }
  }

  function zeichneStatus() {
    const a = statusAnzeige();
    document.querySelectorAll("[data-status]").forEach((box) => {
      box.dataset.zustand = a.zustand;
      const t = box.querySelector(".status-text");
      if (t.textContent !== a.text) t.textContent = a.text;
      box.title = a.zustand === "error" ? a.text : "";
    });

    const laeuft = status.state === "recording";
    $("geraet").dataset.zustand = status.state;
    $("pegel").classList.toggle("aktiv", laeuft);

    const knopf = $("aufnahme-knopf");
    knopf.setAttribute("aria-pressed", laeuft ? "true" : "false");
    knopf.disabled = status.state === "loading" || status.state === "transcribing";
    const schrift = laeuft ? "Aufnahme beenden" : "Aufnahme starten";
    if ($("aufnahme-text").textContent !== schrift) $("aufnahme-text").textContent = schrift;
  }

  /* ------------------------------------------------------------------ */
  /* Seite: Diktat                                                       */
  /* ------------------------------------------------------------------ */

  function modellName(pfad) {
    const m = (snap.models || []).find((x) => gleicherPfad(x.path, pfad));
    if (m) return m.name;
    if (!pfad) return "";
    return String(pfad).split(/[\\/]/).pop();
  }

  function geraetName(engine) {
    const roh = String(engine.device || engine.backend || snap.settings.device || "");
    const klein = roh.toLowerCase();
    if (klein === "cpu") return "Prozessor";
    if (klein === "gpu") return "Grafikkarte";
    const d = (snap.devices || []).find((x) => String(x.name).toLowerCase() === klein);
    if (d) return d.kind === "cpu" ? "Prozessor" : "Grafikkarte";
    return roh || "unbekannt";
  }

  function zeichneDiktat() {
    const s = snap.settings;
    zeichneKappen($("diktat-kappen"), snap.hotkey_labels);
    $("diktat-satz").textContent = s.mode === "toggle"
      ? "Einmal drücken zum Starten, nochmal zum Einfügen."
      : "Halten und sprechen, loslassen zum Einfügen.";

    // Ohne Modell erklärt der Einstiegsblock, was fehlt. Die Fehlerzeile wäre dann doppelt.
    const einstieg = zeichneEinstieg();
    $("engine-fehler").hidden = einstieg || !snap.engine_error;
    $("engine-fehler-text").textContent = snap.engine_error || "";

    const zeile = [el("span", { class: "beschriftung" }, "Modell")];
    if (snap.engine) {
      zeile.push(
        el("span", { class: "modell-name" }, modellName(snap.engine.model_path)),
        el("span", { class: "messwert" }, geraetName(snap.engine)),
        el("span", { class: "messwert" }, `geladen in ${fmtLadezeit(snap.engine.load_ms)}`));
    } else if (status.state === "loading" && s.model) {
      zeile.push(
        el("span", { class: "modell-name" }, modellName(s.model)),
        el("span", { class: "messwert" }, "wird geladen"));
    } else {
      zeile.push(el("span", { class: "leise" }, "Kein Modell geladen"));
    }
    $("modellzeile").replaceChildren(...zeile);

    const neueste = (snap.history || []).slice(0, 3);
    const sig = JSON.stringify(neueste.map((e) => [e.id, e.text])) + tagesSchluessel(new Date());
    if (signatur.zuletzt !== sig) {
      signatur.zuletzt = sig;
      neuAufbauen($("zuletzt"), neueste.map((e) => el("li", null,
        el("p", { class: "kurz-text" }, String(e.text)),
        kopierKnopf(e.text, "textknopf", `k${e.id}`),
        el("span", { class: "kurz-zeit messwert" }, fmtZeitpunkt(e.id)))));
    }
    $("zuletzt").hidden = neueste.length === 0;
    $("zuletzt-leer").hidden = neueste.length > 0;
  }

  /* ------------------------------------------------------------------ */
  /* Seite: Modelle                                                      */
  /* ------------------------------------------------------------------ */

  function mittleresTempo(name) {
    const werte = (snap.history || [])
      .filter((e) => e.model === name)
      .map(tempoVon)
      .filter((x) => x != null);
    if (!werte.length) return null;
    return werte.reduce((a, b) => a + b, 0) / werte.length;
  }

  function detail(name, wert, klasse) {
    return el("div", null,
      el("dt", { class: "beschriftung" }, name),
      el("dd", { class: klasse || null }, wert));
  }

  function kanalzug(m, nr) {
    const s = snap.settings;
    const aktiv = gleicherPfad(m.path, s.model);
    const groesse = Number(m.size_mb);
    const teile = [
      el("div", { class: "kanal-nr messwert", "aria-hidden": "true" },
        el("span", { class: "kanal-led" }),
        String(nr).padStart(2, "0")),
      el("div", { class: "kanal-haupt" },
        el("div", { class: "kanal-name" }, String(m.name)),
        el("div", { class: "kanal-datei messwert", title: String(m.path || "") }, String(m.file))),
      el("div", { class: "kanal-wert" },
        el("span", { class: "beschriftung" }, "Größe"),
        el("span", { class: "messwert" }, Number.isFinite(groesse) ? `${zahl0.format(groesse)} MB` : "unbekannt")),
      el("div", { class: "kanal-wert" },
        el("span", { class: "beschriftung" }, "Quelle"),
        el("span", { title: String(m.source || "") }, String(m.source || "unbekannt"))),
    ];

    if (aktiv) {
      teile.push(el("div", { class: "kanal-aktion" }, el("span", { class: "kanal-aktiv" }, "Aktiv")));
      const e = snap.engine;
      if (e && gleicherPfad(e.model_path, m.path)) {
        const tempo = mittleresTempo(m.name);
        const sprachen = Array.isArray(e.languages) ? e.languages.length : 0;
        teile.push(el("dl", { class: "kanal-details" },
          detail("Architektur", [e.arch, e.variant].filter(Boolean).join(" · ") || "unbekannt"),
          detail("Sprachen", sprachen
            ? `${zahl0.format(sprachen)}${e.language_detect ? ", erkennt automatisch" : ""}`
            : "unbekannt"),
          detail("Ladezeit", fmtLadezeit(e.load_ms), "messwert"),
          detail("Tempo", tempo != null ? `${fmtTempo(tempo)} Echtzeit` : "noch nicht gemessen",
            tempo != null ? "messwert tempo" : "leise")));
      } else {
        const text = status.state === "loading"
          ? "Wird geladen …"
          : (snap.engine_error ? `Nicht geladen: ${snap.engine_error}` : "Noch nicht geladen.");
        teile.push(el("p", { class: "kanal-hinweis hinweis" }, text));
      }
    } else {
      teile.push(el("div", { class: "kanal-aktion" },
        el("button", {
          type: "button",
          class: "knopf klein",
          "data-fokus": `m${m.path}`,
          "aria-label": `${m.name} verwenden`,
          onclick: () => speichere({ model: m.path }),
        }, "Verwenden")));
    }

    return el("article", { class: aktiv ? "kanal aktiv" : "kanal", "aria-label": String(m.name) }, teile);
  }

  function zeichneModelle() {
    const s = snap.settings;
    const modelle = snap.models || [];

    const gpuDa = (snap.devices || []).some((d) => d.kind !== "cpu");
    const gpuZeigen = gpuDa || s.device === "gpu";
    $("geraet-gpu").hidden = !gpuZeigen;
    $("geraet-hinweis").hidden = !gpuZeigen;

    $("modelle-anzahl").textContent = modelle.length
      ? `${modelle.length} ${modelle.length === 1 ? "Datei" : "Dateien"}`
      : "";
    $("modelle-leer").hidden = modelle.length > 0;
    $("modell-liste").hidden = modelle.length === 0;
    $("ordner-pfad").textContent = snap.models_dir || "";

    const ladbar = katalog().length > 0;
    const leerText = ladbar
      ? "Kein Modell gefunden. Lade unten eines oder lege eine .gguf-Datei in den Modellordner."
      : "Kein Modell gefunden. Lege eine .gguf-Datei in den Modellordner.";
    if ($("modelle-leer").textContent !== leerText) $("modelle-leer").textContent = leerText;
    $("laden-block").hidden = !ladbar;
    if (ladbar) zeichneKatalog($("katalog-liste"));

    const sig = JSON.stringify([
      modelle, s.model, snap.engine, snap.engine_error,
      status.state === "loading",
      modelle.map((m) => mittleresTempo(m.name)),
    ]);
    if (signatur.modelle !== sig) {
      signatur.modelle = sig;
      neuAufbauen($("modell-liste"), modelle.map((m, i) => kanalzug(m, i + 1)));
    }
  }

  /* ------------------------------------------------------------------ */
  /* Modelle laden (Katalog)                                             */
  /* ------------------------------------------------------------------ */

  function katalog() {
    return snap && Array.isArray(snap.catalog) ? snap.catalog : [];
  }

  /* Übernimmt den Stand aus Snapshot oder Ereignis. "done" und "cancelled" leeren ihn. */
  function merkeDownload(d) {
    if (!d || d.id == null || (d.state !== "running" && d.state !== "error")) {
      herunterladen = null;
      return;
    }
    const id = String(d.id);
    let fertig = Number(d.done_mb) || 0;
    const alt = herunterladen;
    // Ein Snapshot kann etwas älter sein als das letzte Ereignis. Der Balken läuft nie rückwärts.
    if (alt && alt.id === id && alt.state === "running" && d.state === "running") {
      fertig = Math.max(fertig, alt.done_mb);
    }
    herunterladen = {
      id,
      done_mb: fertig,
      total_mb: Number(d.total_mb) || 0,
      state: d.state,
      message: d.message ? String(d.message) : "",
    };
  }

  function ladeModus(k) {
    if (k.installed) return "vorhanden";
    const d = herunterladen;
    if (d && d.id === String(k.id)) return d.state === "error" ? "fehler" : "laedt";
    if (d && d.state === "running") return "gesperrt";
    return "frei";
  }

  async function starteLaden(id) {
    try {
      uebernimm(await invoke("download_model", { id }));
    } catch (err) {
      melde(fehlertext(err));
    }
  }

  async function brichLadenAb(knopf) {
    knopf.textContent = "Bricht ab …";
    try {
      uebernimm(await invoke("cancel_download"));
    } catch (err) {
      if (knopf.isConnected) knopf.textContent = "Abbrechen";
      melde(`Abbrechen hat nicht geklappt: ${fehlertext(err)}`);
    }
  }

  /* Gerüst einer Katalogzeile. Rechte Seite und zweite Reihe füllt zeichneLadezug. */
  function ladezug(k) {
    const groesse = Number(k.size_mb);
    const herkunft = [k.license, k.source].filter(Boolean).map(String).join(", ");
    const zug = el("article", { class: "kanal ladbar", "data-katalog": String(k.id), "aria-label": String(k.name) },
      el("div", { class: "kanal-nr", "aria-hidden": "true" }),
      el("div", { class: "kanal-haupt" },
        el("div", { class: "kanal-name" }, String(k.name)),
        k.note ? el("p", { class: "kanal-notiz" }, String(k.note)) : null,
        herkunft ? el("div", { class: "kanal-datei messwert", title: herkunft }, herkunft) : null),
      el("div", { class: "kanal-wert" },
        el("span", { class: "beschriftung" }, "Größe"),
        el("span", { class: "messwert" }, Number.isFinite(groesse) ? `${zahl0.format(Math.round(groesse))} MB` : "unbekannt")),
      el("div", { class: "kanal-aktion" }),
      el("div", { class: "kanal-unten", hidden: true }));
    zeichneLadezug(zug, k);
    return zug;
  }

  /* Baut rechte Seite und zweite Reihe für einen Zustand neu. Der Fokus bleibt in der Zeile. */
  function baueLadezug(zug, k, modus) {
    const name = String(k.name);
    const hatteFokus = zug.contains(document.activeElement);
    let oben = [];
    let unten = [];

    if (modus === "vorhanden") {
      oben = [el("span", { class: "kanal-vorhanden" }, "Vorhanden")];
    } else if (modus === "laedt") {
      // Abbrechen sitzt in der zweiten Reihe: ein Doppelklick auf "Laden" trifft es nicht.
      const abbrechen = el("button", { type: "button", class: "knopf klein", "aria-label": `Laden von ${name} abbrechen` }, "Abbrechen");
      abbrechen.addEventListener("click", () => brichLadenAb(abbrechen));
      oben = [el("span", { class: "kanal-prozent messwert", "aria-hidden": "true" })];
      unten = [
        el("div", { class: "lade-stand" },
          el("div", { class: "fortschritt", role: "progressbar", "aria-label": `${name} wird geladen`, "aria-valuemin": "0" },
            el("span", { class: "fortschritt-fuellung" })),
          el("span", { class: "fortschritt-text messwert" })),
        abbrechen,
      ];
    } else if (modus === "fehler") {
      oben = [el("span", { class: "kanal-fehler" }, "Fehler")];
      unten = [
        el("p", { class: "lade-fehler", role: "alert" }, herunterladen.message || "Das Laden ist fehlgeschlagen."),
        el("button", {
          type: "button",
          class: "knopf klein",
          "aria-label": `${name}: nochmal versuchen`,
          onclick: () => starteLaden(k.id),
        }, "Nochmal versuchen"),
      ];
    } else {
      oben = [el("button", {
        type: "button",
        class: "knopf klein",
        "aria-label": `${name} laden`,
        onclick: () => starteLaden(k.id),
      }, "Laden")];
    }

    zug.querySelector(".kanal-nr").replaceChildren(
      el("span", { class: "kanal-led" }),
      symbol(modus === "vorhanden" ? "haken" : "laden"));
    zug.querySelector(".kanal-aktion").replaceChildren(...oben);
    const reihe = zug.querySelector(".kanal-unten");
    reihe.replaceChildren(...unten);
    reihe.hidden = unten.length === 0;
    // Ohne Knopf (etwa "Vorhanden") fängt die Liste den Fokus auf.
    if (hatteFokus) (zug.querySelector("button:not(:disabled)") || zug.parentElement).focus({ preventScroll: true });
  }

  function setzeFortschritt(zug, k, d) {
    const balken = zug.querySelector(".fortschritt");
    const schrift = zug.querySelector(".fortschritt-text");
    if (!balken || !schrift) return;
    const fertig = Math.max(0, Math.round(d.done_mb));
    let gesamt = Math.round(d.total_mb);
    if (!(gesamt > 0)) gesamt = Math.round(Number(k.size_mb)) || 0;
    const stand = gesamt > 0 ? Math.min(fertig, gesamt) : fertig;
    const text = gesamt > 0
      ? `${zahl0.format(stand)} von ${zahl0.format(gesamt)} MB`
      : `${zahl0.format(stand)} MB`;
    if (schrift.textContent === text) return;
    schrift.textContent = text;

    const anteil = gesamt > 0 ? stand / gesamt : 0;
    balken.firstElementChild.style.transform = `scaleX(${anteil.toFixed(4)})`;
    balken.setAttribute("aria-valuetext", text);
    if (gesamt > 0) {
      balken.setAttribute("aria-valuemax", String(gesamt));
      balken.setAttribute("aria-valuenow", String(stand));
    } else {
      balken.removeAttribute("aria-valuemax");               // Größe unbekannt
      balken.removeAttribute("aria-valuenow");
    }
    zug.querySelector(".kanal-prozent").textContent = gesamt > 0 ? `${zahl0.format(Math.floor(anteil * 100))} %` : "";
  }

  /* Gleicht eine Katalogzeile mit dem Stand ab. Neu gebaut wird nur, wenn der Zustand wechselt. */
  function zeichneLadezug(zug, k) {
    const modus = ladeModus(k);
    const bau = modus === "gesperrt"
      ? "frei"
      : (modus === "fehler" ? `fehler\u0001${herunterladen.message}` : modus);
    if (zug.dataset.bau !== bau) {
      zug.dataset.bau = bau;
      baueLadezug(zug, k, modus);
    }
    if (zug.dataset.modus !== modus) zug.dataset.modus = modus;

    if (modus === "frei" || modus === "gesperrt") {
      const knopf = zug.querySelector(".kanal-aktion button");
      const gesperrt = modus === "gesperrt";
      if (knopf.disabled !== gesperrt) {
        knopf.disabled = gesperrt;
        knopf.title = gesperrt ? "Es lädt schon ein anderes Modell." : "";
      }
    } else if (modus === "laedt") {
      setzeFortschritt(zug, k, herunterladen);
    }
  }

  /* Alle sichtbaren und verdeckten Katalogzeilen abgleichen, ohne die Listen neu zu bauen. */
  function zeichneLadestand(wurzel) {
    const liste = katalog();
    (wurzel || document).querySelectorAll("[data-katalog]").forEach((zug) => {
      const k = liste.find((x) => String(x.id) === zug.dataset.katalog);
      if (k) zeichneLadezug(zug, k);
    });
  }

  function zeichneKatalog(behaelter) {
    const liste = katalog();
    const sig = JSON.stringify(liste.map((k) => [k.id, k.name, k.note, k.size_mb, k.license, k.source]));
    if (behaelter.dataset.sig !== sig) {
      behaelter.dataset.sig = sig;
      neuAufbauen(behaelter, liste.map(ladezug));
    } else {
      zeichneLadestand(behaelter);
    }
  }

  /* Diktat-Seite: Einstieg, solange es noch kein Sprachmodell gibt. */
  function zeichneEinstieg() {
    const box = $("einstieg");
    const zeigen = katalog().length > 0 && (snap.models || []).length === 0;
    if (zeigen) {
      zeichneKatalog($("einstieg-liste"));
    } else if (!box.hidden && box.contains(document.activeElement)) {
      box.hidden = true;                                       // Fokus nicht im verschwundenen Block lassen
      $("titel-diktat").focus({ preventScroll: true });
    }
    box.hidden = !zeigen;
    return zeigen;
  }

  function ladezeilenSichtbar() {
    return seite === "modelle" || (seite === "diktat" && !$("einstieg").hidden);
  }

  /* ------------------------------------------------------------------ */
  /* Seite: Einstellungen                                                */
  /* ------------------------------------------------------------------ */

  function istTextfeld(f) {
    return f.tagName === "TEXTAREA" || (f.tagName === "INPUT" && (f.type === "text" || f.type === "number"));
  }

  function zeichneMikrofone() {
    const s = snap.settings;
    const liste = snap.microphones || [];
    const aktuell = s.microphone || "";
    const sig = JSON.stringify([liste, aktuell]);
    if (signatur.mikrofone === sig) return;
    signatur.mikrofone = sig;
    const optionen = [el("option", { value: "" }, "Windows-Standard")];
    for (const name of liste) optionen.push(el("option", { value: name }, name));
    if (aktuell && !liste.includes(aktuell)) {
      optionen.push(el("option", { value: aktuell }, `${aktuell} (nicht angeschlossen)`));
    }
    $("mikrofon").replaceChildren(...optionen);
  }

  function zeichneFelder() {
    const s = snap.settings;

    if (s.language && !SPRACHEN.includes(s.language) && !$("sprache").querySelector(`option[value="${CSS.escape(s.language)}"]`)) {
      $("sprache").append(el("option", { value: s.language }, s.language));
    }

    document.querySelectorAll("[data-feld]").forEach((f) => {
      const wert = s[f.dataset.feld];
      if (f.type === "radio") {
        f.checked = f.value === String(wert);
      } else if (f.type === "checkbox") {
        f.checked = Boolean(wert);
      } else if (istTextfeld(f)) {
        // Felder, in denen gerade getippt wird, bleiben unberührt.
        if (document.activeElement !== f) f.value = wert == null ? "" : String(wert);
      } else {
        f.value = wert == null ? "" : String(wert);
      }
    });

    const ueberAblage = s.paste_method === "ctrl_v";
    $("zeile-zwischenablage").classList.toggle("gedimmt", !ueberAblage);
    $("zeile-zwischenablage").querySelector("input").disabled = !ueberAblage;

    $("modus-hinweis").textContent = s.mode === "toggle"
      ? "Einmal drücken zum Starten, nochmal zum Einfügen."
      : "Halten und sprechen, loslassen zum Einfügen.";
  }

  function zeichneKuerzel() {
    const paare = [["hotkey", snap.hotkey_labels], ["cancel_key", snap.cancel_labels]];
    for (const [ziel, labels] of paare) {
      const zeile = document.querySelector(`[data-kuerzel="${ziel}"]`);
      const lauscht = lauschZiel === ziel;
      zeichneKappen(zeile.querySelector(".kappen"), labels);
      zeile.classList.toggle("lauscht", lauscht);
      const hinweis = zeile.querySelector(".lausch-text");
      const text = lauscht
        ? (ziel === "hotkey" ? "Drück jetzt das neue Kürzel …" : "Drück jetzt die neue Abbruchtaste …")
        : "";
      if (hinweis.textContent !== text) hinweis.textContent = text;
      const knopf = zeile.querySelector("[data-kuerzel-knopf]");
      const schrift = lauscht ? "Abbrechen" : "Ändern";
      if (knopf.textContent !== schrift) knopf.textContent = schrift;
      knopf.disabled = !lauscht && lauschZiel != null;
    }
  }

  async function kuerzelKlick(ziel) {
    if (lauschZiel === ziel) {
      lauschZiel = null;
      zeichneKuerzel();
      try { await invoke("cancel_capture"); } catch (err) { melde(fehlertext(err)); }
      return;
    }
    if (lauschZiel) return;
    lauschZiel = ziel;
    zeichneKuerzel();
    try {
      await invoke("capture_hotkey", { target: ziel });
    } catch (err) {
      lauschZiel = null;
      zeichneKuerzel();
      melde(fehlertext(err));
    }
  }

  function lauschenBeenden() {
    if (!lauschZiel) return;
    lauschZiel = null;
    if (snap) zeichneKuerzel();
    invoke("cancel_capture").catch(() => {});
  }

  /* ---------- Eigene Wörter ---------- */

  function leseVokabular() {
    return $("vokabular").value.split(/\r?\n/).map((z) => z.trim()).filter(Boolean);
  }

  function zeichneVokabular() {
    const feld = $("vokabular");
    if (document.activeElement === feld) return;
    const soll = (snap.settings.vocabulary || []).join("\n");
    if (feld.value !== soll) feld.value = soll;
  }

  /* ---------- Ersetzungen ---------- */

  function ersZeile(from, to) {
    const von = el("input", { type: "text", class: "eingabe", placeholder: "Gehört", "aria-label": "Gehört", spellcheck: "false", autocomplete: "off" });
    const zu = el("input", { type: "text", class: "eingabe", placeholder: "Schreiben als", "aria-label": "Schreiben als", spellcheck: "false", autocomplete: "off" });
    von.value = from || "";
    zu.value = to || "";
    const weg = el("button", { type: "button", class: "symbolknopf", "aria-label": "Ersetzung löschen" }, symbol("kreuz"));
    const zeile = el("div", { class: "ers-zeile" },
      von,
      el("span", { class: "pfeil", "aria-hidden": "true" }, symbol("pfeil")),
      zu,
      weg);
    weg.addEventListener("click", () => {
      zeile.remove();
      $("ers-kopf").hidden = !$("ersetzungen").children.length;
      $("ers-neu").focus();
      speichereErsetzungen();
    });
    return zeile;
  }

  function leseErsetzungen() {
    return [...$("ersetzungen").querySelectorAll(".ers-zeile")]
      .map((z) => {
        const [von, zu] = z.querySelectorAll("input");
        return { from: von.value.trim(), to: zu.value };
      })
      .filter((r) => r.from !== "");
  }

  function speichereErsetzungen() {
    entprellenAbbrechen("replacements");
    speichere({ replacements: leseErsetzungen() });
  }

  function zeichneErsetzungen() {
    const liste = $("ersetzungen");
    if (liste.contains(document.activeElement)) return;      // wird gerade bearbeitet
    const soll = (snap.settings.replacements || []).filter((r) => r && (r.from || r.to));
    liste.replaceChildren(...soll.map((r) => ersZeile(r.from, r.to)));
    $("ers-kopf").hidden = soll.length === 0;
  }

  function zeichneEinstellungen() {
    zeichneMikrofone();
    zeichneFelder();
    zeichneKuerzel();
    zeichneVokabular();
    zeichneErsetzungen();
  }

  /* ---------- Speichern ---------- */

  function leseZahl(feld) {
    const n = Number.parseInt(feld.value, 10);
    if (!Number.isFinite(n)) return null;
    const min = feld.min === "" ? -Infinity : Number(feld.min);
    const max = feld.max === "" ? Infinity : Number(feld.max);
    return Math.min(max, Math.max(min, n));
  }

  /* Klont die Einstellungen, ändert die Felder, speichert, zeichnet das Ergebnis.
     Aufrufe laufen nacheinander, damit sich zwei Änderungen nicht überholen. */
  function speichere(aenderung) {
    speicherKette = speicherKette.then(async () => {
      if (!snap) return;
      const settings = JSON.parse(JSON.stringify(snap.settings));
      let geaendert = false;
      for (const [k, v] of Object.entries(aenderung)) {
        if (JSON.stringify(settings[k]) !== JSON.stringify(v)) {
          settings[k] = v;
          geaendert = true;
        }
      }
      if (!geaendert) return;
      try {
        uebernimm(await invoke("save_settings", { settings }));
        zeigeGespeichert();
      } catch (err) {
        melde(fehlertext(err));
        try { uebernimm(await invoke("get_state")); } catch (_) { zeichneAlles(); }
      }
    });
    return speicherKette;
  }

  /* ------------------------------------------------------------------ */
  /* Seite: Verlauf                                                      */
  /* ------------------------------------------------------------------ */

  function verlaufEintrag(e) {
    const tempo = tempoVon(e);
    const loeschen = el("button", {
      type: "button",
      class: "knopf klein",
      "data-fokus": `l${e.id}`,
    }, symbol("loeschen"), "Löschen");
    loeschen.addEventListener("click", async () => {
      // Fokus soll nach dem Löschen auf dem nächsten Eintrag landen.
      const nachbar = loeschen.closest(".eintrag").nextElementSibling || loeschen.closest(".eintrag").previousElementSibling;
      const marke = nachbar ? nachbar.querySelector("[data-fokus^='l']") : null;
      if (marke) loeschen.setAttribute("data-fokus", marke.getAttribute("data-fokus"));
      try {
        uebernimm(await invoke("delete_history", { id: e.id }));
      } catch (err) {
        melde(`Löschen hat nicht geklappt: ${fehlertext(err)}`);
      }
    });

    const daten = [
      datenpaar("Zeit", fmtZeitpunkt(e.id)),
      datenpaar("Dauer", fmtDauer(e.audio_ms)),
      datenpaar("Tempo", tempo != null ? fmtTempo(tempo) : "unbekannt"),
    ];
    if (e.model) daten.push(datenpaar("Modell", String(e.model)));

    return el("article", { class: "eintrag" },
      el("p", { class: "eintrag-text" }, String(e.text)),
      el("div", { class: "eintrag-knoepfe" },
        kopierKnopfGross(e),
        loeschen),
      el("div", { class: "eintrag-daten" }, daten));
  }

  function kopierKnopfGross(e) {
    const knopf = kopierKnopf(e.text, "knopf klein", `k${e.id}`);
    knopf.prepend(symbol("kopieren"));
    return knopf;
  }

  function zeichneVerlauf() {
    const h = snap.history || [];
    $("verlauf-anzahl").textContent = h.length
      ? `${zahl0.format(h.length)} ${h.length === 1 ? "Eintrag" : "Einträge"}`
      : "";
    $("verlauf-leer").hidden = h.length > 0;
    if (!h.length) leerenOffen = false;
    $("leeren-knopf").hidden = leerenOffen || h.length === 0;
    $("leeren-frage").hidden = !leerenOffen;
    $("leeren-text").textContent = h.length === 1
      ? "Den einen Eintrag löschen?"
      : `Alle ${zahl0.format(h.length)} Einträge löschen?`;

    const sig = JSON.stringify(h.map((e) => [e.id, e.text, e.audio_ms, e.took_ms, e.model])) + tagesSchluessel(new Date());
    if (signatur.verlauf !== sig) {
      signatur.verlauf = sig;
      neuAufbauen($("verlauf-liste"), h.map(verlaufEintrag));
    }
  }

  /* ------------------------------------------------------------------ */
  /* Alles zeichnen                                                      */
  /* ------------------------------------------------------------------ */

  function zeichneInfo() {
    const v = snap.version ? String(snap.version) : "";
    $("info-version").textContent = v || "unbekannt";
    $("leiste-version").textContent = v ? `Version ${v}` : "";
  }

  function zeichneAlles() {
    if (!snap) return;
    zeichneStatus();
    zeichneDiktat();
    zeichneModelle();
    zeichneEinstellungen();
    zeichneVerlauf();
    zeichneInfo();
  }

  function uebernimm(neu) {
    if (!neu || typeof neu !== "object") return;
    snap = neu;
    if (neu.status && neu.status.state) status = neu.status;
    merkeDownload(neu.download);
    // Auf dem Mac heisst die Einfuege-Taste Command+V.
    const ablage = document.querySelector('#methode option[value="ctrl_v"]');
    if (ablage) {
      ablage.textContent = neu.platform === "macos"
        ? "Über Zwischenablage (Command+V)"
        : "Über Zwischenablage (Strg+V)";
    }
    zeichneAlles();
  }

  async function lade() {
    const nr = ++ladeNr;
    try {
      const neu = await invoke("get_state");
      if (nr === ladeNr) uebernimm(neu);
    } catch (err) {
      melde(`Der Zustand konnte nicht geladen werden: ${fehlertext(err)}`);
    }
  }

  /* ------------------------------------------------------------------ */
  /* Navigation                                                          */
  /* ------------------------------------------------------------------ */

  function zeigeSeite(name, fokusAufTitel) {
    if (name !== "einstellungen") lauschenBeenden();
    if (name !== "verlauf" && leerenOffen) {
      leerenOffen = false;
      if (snap) zeichneVerlauf();
    }
    seite = name;
    document.querySelectorAll(".seite").forEach((s) => { s.hidden = s.dataset.seite !== name; });
    document.querySelectorAll(".nav-punkt").forEach((n) => {
      if (n.dataset.seite === name) n.setAttribute("aria-current", "page");
      else n.removeAttribute("aria-current");
    });
    $("inhalt").scrollTop = 0;
    if (fokusAufTitel) {
      const titel = $(`titel-${name}`);
      if (titel) titel.focus({ preventScroll: true });
    }
  }

  /* ------------------------------------------------------------------ */
  /* Ereignisse der Oberfläche                                           */
  /* ------------------------------------------------------------------ */

  function verdrahte() {
    const nav = document.querySelector(".nav");
    nav.addEventListener("click", (e) => {
      const punkt = e.target.closest(".nav-punkt");
      if (punkt) zeigeSeite(punkt.dataset.seite, false);
    });
    nav.addEventListener("keydown", (e) => {
      if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
      const punkte = [...nav.querySelectorAll(".nav-punkt")];
      const i = punkte.indexOf(document.activeElement);
      if (i < 0) return;
      e.preventDefault();
      punkte[(i + (e.key === "ArrowDown" ? 1 : punkte.length - 1)) % punkte.length].focus();
    });

    document.addEventListener("click", (e) => {
      const sprung = e.target.closest("[data-gehe]");
      if (sprung) zeigeSeite(sprung.dataset.gehe, true);
      const kuerzel = e.target.closest("[data-kuerzel-knopf]");
      if (kuerzel) kuerzelKlick(kuerzel.dataset.kuerzelKnopf);
    });

    $("aufnahme-knopf").addEventListener("click", () => {
      invoke("toggle_record").catch((err) => melde(fehlertext(err)));
    });

    $("ordner-knopf").addEventListener("click", () => {
      invoke("open_models_dir").catch((err) => melde(`Der Ordner ließ sich nicht öffnen: ${fehlertext(err)}`));
    });

    $("beenden-knopf").addEventListener("click", () => {
      invoke("quit_app").catch((err) => melde(fehlertext(err)));
    });

    $("meldung-zu").addEventListener("click", () => {
      clearTimeout(meldungTimer);
      $("meldung").hidden = true;
    });

    /* Einstellungen: jede Änderung sofort speichern */
    document.addEventListener("change", (e) => {
      const f = e.target;
      if (!f || !f.dataset || !f.dataset.feld || !snap) return;
      const k = f.dataset.feld;
      let wert;
      if (f.type === "checkbox") {
        wert = f.checked;
      } else if (f.type === "radio") {
        if (!f.checked) return;
        wert = f.value;
      } else if (f.dataset.typ === "zahl") {
        wert = leseZahl(f);
        if (wert == null) {
          f.value = snap.settings[k] == null ? "" : String(snap.settings[k]);
          return;
        }
        f.value = String(wert);
      } else {
        wert = f.value;
      }
      entprellenAbbrechen(k);
      speichere({ [k]: wert });
    });

    document.addEventListener("input", (e) => {
      const f = e.target;
      if (!f || !f.dataset || f.dataset.typ !== "zahl" || !f.dataset.feld) return;
      const k = f.dataset.feld;
      entprelle(k, () => {
        const wert = leseZahl(f);
        if (wert != null) speichere({ [k]: wert });
      });
    });

    const vokabular = $("vokabular");
    vokabular.addEventListener("input", () => {
      entprelle("vocabulary", () => speichere({ vocabulary: leseVokabular() }));
    });
    vokabular.addEventListener("blur", () => {
      entprellenAbbrechen("vocabulary");
      speichere({ vocabulary: leseVokabular() });
    });

    const ersetzungen = $("ersetzungen");
    ersetzungen.addEventListener("input", () => {
      entprelle("replacements", () => speichere({ replacements: leseErsetzungen() }));
    });
    ersetzungen.addEventListener("change", speichereErsetzungen);

    $("ers-neu").addEventListener("click", () => {
      const zeile = ersZeile("", "");
      ersetzungen.append(zeile);
      $("ers-kopf").hidden = false;
      zeile.querySelector("input").focus();
    });

    /* Verlauf leeren, mit Rückfrage in der Oberfläche */
    $("leeren-knopf").addEventListener("click", () => {
      leerenOffen = true;
      zeichneVerlauf();
      $("leeren-nein").focus();
    });
    $("leeren-nein").addEventListener("click", () => {
      leerenOffen = false;
      zeichneVerlauf();
      $("leeren-knopf").focus();
    });
    $("leeren-ja").addEventListener("click", async () => {
      leerenOffen = false;
      try {
        uebernimm(await invoke("clear_history"));
      } catch (err) {
        melde(`Leeren hat nicht geklappt: ${fehlertext(err)}`);
        zeichneVerlauf();
      }
      $("titel-verlauf").focus({ preventScroll: true });
    });
    $("leeren-frage").addEventListener("keydown", (e) => {
      if (e.key === "Escape") $("leeren-nein").click();
    });
  }

  /* ------------------------------------------------------------------ */
  /* Start                                                               */
  /* ------------------------------------------------------------------ */

  /* Zwischenstand der laufenden Aufnahme (Ereignis "partial"). */
  function zeigeMitschrift(text, gekuerzt) {
    const box = $("geraet-live");
    box.classList.remove("fertigt");
    text = text ? String(text) : "";
    box.hidden = !text;
    $("live-text").textContent = text ? (gekuerzt ? "… " : "") + text : "";
  }

  function horche(name, fn) {
    Promise.resolve(listen(name, fn)).catch((err) => {
      melde(`Ereignis „${name}“ nicht erreichbar: ${fehlertext(err)}`);
    });
  }

  function start() {
    verdrahte();
    zeigeSeite(seite, false);

    horche("status", (e) => {
      const p = e && e.payload;
      if (!p || !p.state) return;
      const vorher = status.state;
      status = { state: p.state, message: p.message || "" };
      if (snap) snap.status = status;
      zeichneStatus();
      // Mitschrift: beim Schreiben gedimmt stehen lassen, sonst bei jedem Wechsel leeren.
      if (vorher !== status.state) {
        if (status.state === "transcribing") $("geraet-live").classList.add("fertigt");
        else zeigeMitschrift("");
      }
      // Wechsel von oder zu "Lädt Modell" ändert auch Modellzeile und Kanalzug.
      if (snap && vorher !== status.state && (vorher === "loading" || status.state === "loading")) {
        zeichneDiktat();
        zeichneModelle();
      }
    });

    horche("level", (e) => {
      pegel.setze(e ? e.payload : 0);
    });

    horche("partial", (e) => {
      const p = e && e.payload;
      if (p && status.state === "recording") zeigeMitschrift(p.text, p.cut);
    });

    /* Fortschritt beim Laden eines Modells: nur die betroffenen Zeilen anfassen. */
    horche("download", (e) => {
      const p = e && e.payload;
      if (!p || p.id == null) return;
      const vorher = herunterladen ? herunterladen.state : "";
      if (p.state === "done") {
        // Gleich als vorhanden zeigen, "refresh" bringt danach den echten Stand.
        const k = katalog().find((x) => String(x.id) === String(p.id));
        if (k) k.installed = true;
      }
      merkeDownload(p);
      zeichneLadestand();
      const jetzt = herunterladen ? herunterladen.state : "";
      if (vorher === jetzt) return;
      zeichneStatus();
      if (jetzt === "error" && !ladezeilenSichtbar()) {
        const k = katalog().find((x) => String(x.id) === herunterladen.id);
        const grund = herunterladen.message || "Das Laden ist fehlgeschlagen.";
        melde(k ? `${k.name}: ${grund}` : grund);
      }
    });

    horche("refresh", () => {
      if (lauschZiel) {
        lauschZiel = null;                           // neues Kürzel ist übernommen
        if (snap) zeichneKuerzel();
      }
      lade();
    });

    lade();
  }

  start();

  /* ------------------------------------------------------------------ */
  /* Mock für die Vorschau im Browser (ohne window.__TAURI__)            */
  /* ------------------------------------------------------------------ */

  function erzeugeMock() {
    const hoerer = new Map();
    const sende = (name, payload) => {
      (hoerer.get(name) || []).slice().forEach((fn) => {
        try { fn({ event: name, payload }); } catch (err) { console.error(err); }
      });
    };

    const ORDNER = "C:\\Users\\du\\AppData\\Roaming\\FreeSpeech\\models";
    const HF = "C:\\Users\\du\\.cache\\huggingface\\hub";
    const jetzt = Date.now();
    const MIN = 60000;

    // ?leer=1 zeigt einen frischen Rechner ohne Modell, ?fehler=1 lässt den ersten Download scheitern.
    const wunsch = new URLSearchParams(window.location.search);
    const LEER = wunsch.get("leer") === "1";
    let fehlerOffen = wunsch.get("fehler") === "1";

    // Was ein Katalogeintrag im Modellordner ablegt.
    const KATALOG_DATEI = {
      primeline: { file: "parakeet-primeline-q8_0.gguf", name: "Parakeet Primeline" },
      canary: { file: "canary-180m-flash-q8_0.gguf", name: "Canary 180m Flash" },
    };

    const ENGINES = {
      "parakeet-primeline-q8_0.gguf": { arch: "parakeet", variant: "tdt 0.6b", load_ms: 850, sprachen: 25 },
      "nemotron-3.5-asr-streaming-0.6b-q8_0.gguf": { arch: "nemotron", variant: "streaming 0.6b", load_ms: 1240, sprachen: 8 },
      "canary-180m-flash-q8_0.gguf": { arch: "canary", variant: "180m flash", load_ms: 410, sprachen: 4 },
    };
    const CODES = ["de-DE", "en-US", "fr-FR", "es-ES", "it-IT", "nl-NL", "pt-PT", "pl-PL", "tr-TR", "ru-RU", "uk-UA", "ja-JP",
      "sv-SE", "da-DK", "fi-FI", "cs-CZ", "sk-SK", "hu-HU", "ro-RO", "bg-BG", "hr-HR", "el-GR", "et-EE", "lv-LV", "lt-LT"];

    const z = {
      version: "0.1.0",
      settings: {
        model: `${ORDNER}\\parakeet-primeline-q8_0.gguf`,
        device: "cpu",
        threads: 0,
        language: "de",
        hotkey: "ctrl_left+super_left",
        cancel_key: "escape",
        mode: "ptt",
        microphone: "",
        paste_method: "ctrl_v",
        restore_clipboard: true,
        trailing_space: true,
        sounds: false,
        overlay: true,
        live_preview: true,
        history_limit: 50,
        autostart: false,
        start_hidden: false,
        replacements: [
          { from: "Rodecaster", to: "RODECaster" },
          { from: "da vinci resolve", to: "DaVinci Resolve" },
        ],
        vocabulary: ["RODECaster", "Octane", "Cinema 4D", "Keyframe"],
      },
      status: { state: "idle", message: "" },
      models: [
        { path: `${ORDNER}\\parakeet-primeline-q8_0.gguf`, name: "Parakeet Primeline", file: "parakeet-primeline-q8_0.gguf", size_mb: 652.4, source: "Modellordner" },
        { path: `${HF}\\nemotron-3.5-asr-streaming-0.6b-q8_0.gguf`, name: "Nemotron 3.5 Asr Streaming 0.6b", file: "nemotron-3.5-asr-streaming-0.6b-q8_0.gguf", size_mb: 701.9, source: "HuggingFace-Cache" },
      ],
      // Canary fehlt oben mit Absicht, damit sich das Laden in der Vorschau zeigen lässt.
      catalog: [
        { id: "primeline", name: "Parakeet Primeline", note: "Deutsch. Beste Wahl für deutsche Diktate.", size_mb: 705, license: "CC BY 4.0", source: "handy-computer/parakeet-primeline-gguf", installed: false },
        { id: "canary", name: "Canary 180M Flash", note: "Deutsch, Englisch, Französisch, Spanisch. Klein und schnell.", size_mb: 198, license: "CC BY 4.0", source: "handy-computer/canary-180m-flash-gguf", installed: false },
      ],
      download: null,
      devices: [
        { name: "cpu", kind: "cpu", description: "Prozessor, 16 Threads" },
        { name: "vulkan0", kind: "gpu", description: "Grafikkarte über Vulkan" },
      ],
      microphones: [
        "Mikrofon (RODECaster Pro Stereo)",
        "Mikrofon (RODECaster Pro Multitrack)",
        "Mikrofon (HD Webcam)",
      ],
      engine: null,
      engine_error: null,
      history: [
        { id: jetzt - 4 * MIN, text: "Schneid den Einstieg bitte auf die ersten drei Sekunden runter und zieh den Titel einen halben Takt nach vorn.", audio_ms: 7400, took_ms: 2150, model: "Parakeet Primeline" },
        { id: jetzt - 52 * MIN, text: "Notiz für morgen: Das Octane-Material für die Dose braucht noch mehr Rauheit an den Kanten, sonst wirkt das Etikett aufgeklebt.", audio_ms: 9800, took_ms: 3050, model: "Parakeet Primeline" },
        { id: jetzt - 26 * 60 * MIN, text: "Hallo Anna, die zweite Fassung liegt im Ordner. Sag mir bis Freitag, ob der Ton so passt.", audio_ms: 6100, took_ms: 1700, model: "Parakeet Primeline" },
        { id: jetzt - 3 * 24 * 60 * MIN, text: "Das Set am Samstag startet um dreiundzwanzig Uhr, Soundcheck ist eine Stunde vorher.", audio_ms: 5600, took_ms: 2900, model: "Canary 180m Flash" },
      ],
      hotkey_labels: ["Strg links", "Win links"],
      cancel_labels: ["Esc"],
      models_dir: ORDNER,
    };

    function engineFuer(pfad) {
      const m = z.models.find((x) => x.path === pfad);
      if (!m) return null;
      const e = ENGINES[m.file] || { arch: "unbekannt", variant: "", load_ms: 900, sprachen: 1 };
      return {
        model_path: m.path,
        arch: e.arch,
        variant: e.variant,
        backend: z.settings.device === "gpu" ? "vulkan" : "cpu",
        device: z.settings.device,
        languages: CODES.slice(0, e.sprachen),
        language_detect: e.sprachen > 4,
        load_ms: z.settings.device === "gpu" ? e.load_ms * 3 : e.load_ms,
      };
    }
    if (LEER) {
      z.models = [];
      z.history = [];
      z.settings.model = "";
      z.engine_error = "Kein Modell gefunden. Lege eine .gguf-Datei in den Modellordner.";
    }
    z.engine = engineFuer(z.settings.model);

    const kopie = () => {
      // Wie im Kern: vorhanden ist, was als Datei gefunden wird.
      z.catalog.forEach((k) => {
        k.installed = z.models.some((m) => m.file === KATALOG_DATEI[k.id].file);
      });
      return JSON.parse(JSON.stringify(z));
    };
    const setzeStatus = (state, message) => {
      z.status = { state, message: message || "" };
      sende("status", { state, message: message || "" });
    };

    const SAETZE = [
      "Das ist ein Probediktat aus der Vorschau, nichts davon wurde wirklich aufgenommen.",
      "Leg die Atmo eine Spur tiefer und nimm bei zwölf Sekunden die Höhen etwas zurück.",
      "Erinnerung: Angebot für den Drei-D-Clip heute noch rausschicken.",
    ];
    let satzNr = 0;
    let pegelTimer = 0;
    let aufnahmeStart = 0;
    let ladeTimer = 0;
    let downloadTimer = 0;
    let fangEnde = null;

    function ladeModell() {
      clearTimeout(ladeTimer);
      z.engine = null;
      z.engine_error = null;
      setzeStatus("loading");
      ladeTimer = setTimeout(() => {
        z.engine = engineFuer(z.settings.model);
        if (!z.engine) z.engine_error = "Die Modelldatei wurde nicht gefunden.";
        setzeStatus(z.engine ? "idle" : "error", z.engine ? "" : z.engine_error);
        sende("refresh");
      }, 1100);
    }

    function beendeFang() {
      if (fangEnde) fangEnde();
      fangEnde = null;
    }

    const TASTEN = {
      ControlLeft: ["Strg links", "ctrl_left"], ControlRight: ["Strg rechts", "ctrl_right"],
      MetaLeft: ["Win links", "super_left"], MetaRight: ["Win rechts", "super_right"],
      AltLeft: ["Alt links", "alt_left"], AltRight: ["Alt Gr", "alt_right"],
      ShiftLeft: ["Umschalt links", "shift_left"], ShiftRight: ["Umschalt rechts", "shift_right"],
      Space: ["Leertaste", "space"], Escape: ["Esc", "escape"], Enter: ["Eingabe", "enter"],
      Tab: ["Tab", "tab"], Backspace: ["Rück", "backspace"], CapsLock: ["Feststell", "caps_lock"],
    };
    function taste(code) {
      if (TASTEN[code]) return TASTEN[code];
      const kurz = code.replace(/^Key/, "").replace(/^Digit/, "");
      return [kurz, kurz.toLowerCase()];
    }

    const befehle = {
      async get_state() {
        return kopie();
      },

      async save_settings({ settings }) {
        if (!settings || typeof settings !== "object") throw "Ungültige Einstellungen.";
        if (!Number.isInteger(settings.threads) || settings.threads < 0) throw "Threads muss eine ganze Zahl ab 0 sein.";
        if (!Number.isInteger(settings.history_limit) || settings.history_limit < 0 || settings.history_limit > 500) {
          throw "Der Verlauf fasst 0 bis 500 Einträge.";
        }
        const neuLaden = settings.model !== z.settings.model || settings.device !== z.settings.device;
        z.settings = JSON.parse(JSON.stringify(settings));
        z.history = z.history.slice(0, z.settings.history_limit);
        if (neuLaden) ladeModell();
        return kopie();
      },

      async capture_hotkey({ target }) {
        beendeFang();
        const codes = [];
        const runter = (e) => {
          e.preventDefault();
          e.stopPropagation();
          if (!codes.includes(e.code)) codes.push(e.code);
        };
        const hoch = (e) => {
          if (!codes.length) return;
          e.preventDefault();
          e.stopPropagation();
          beendeFang();
          const paare = codes.map(taste);
          if (target === "cancel_key") {
            z.settings.cancel_key = paare.map((p) => p[1]).join("+");
            z.cancel_labels = paare.map((p) => p[0]);
          } else {
            z.settings.hotkey = paare.map((p) => p[1]).join("+");
            z.hotkey_labels = paare.map((p) => p[0]);
          }
          sende("refresh");
        };
        window.addEventListener("keydown", runter, true);
        window.addEventListener("keyup", hoch, true);
        fangEnde = () => {
          window.removeEventListener("keydown", runter, true);
          window.removeEventListener("keyup", hoch, true);
        };
      },

      async cancel_capture() {
        beendeFang();
      },

      async toggle_record() {
        const zustand = z.status.state;
        if (zustand === "idle" || zustand === "error") {
          if (!z.engine) {
            setzeStatus("error", "Kein Modell geladen. Wähl unter Modelle eines aus.");
            return;
          }
          aufnahmeStart = performance.now();
          setzeStatus("recording");
          let t = 0;
          pegelTimer = setInterval(() => {
            t += 0.033;
            const silbe = Math.max(0, Math.sin(t * 7.3)) * (0.55 + 0.45 * Math.sin(t * 1.7));
            const atem = Math.sin(t * 0.9) > -0.6 ? 1 : 0.06;
            let v = 0.015 + 0.36 * silbe * atem * (0.7 + Math.random() * 0.5);
            if (Math.random() < 0.012) v = 0.78 + Math.random() * 0.22;
            sende("level", Math.min(1, v));
          }, 33);
        } else if (zustand === "recording") {
          clearInterval(pegelTimer);
          const audio = Math.max(600, Math.round(performance.now() - aufnahmeStart));
          const took = Math.round(audio / (2.8 + Math.random()));
          setzeStatus("transcribing");
          setTimeout(() => {
            const text = SAETZE[satzNr++ % SAETZE.length];
            const m = z.models.find((x) => z.engine && x.path === z.engine.model_path);
            z.history.unshift({ id: Date.now(), text, audio_ms: audio, took_ms: took, model: m ? m.name : "" });
            z.history = z.history.slice(0, z.settings.history_limit);
            const feld = document.activeElement;
            if (feld && feld.id === "probefeld" && z.settings.paste_method !== "none") {
              feld.setRangeText(text + (z.settings.trailing_space ? " " : ""), feld.selectionStart, feld.selectionEnd, "end");
            }
            setzeStatus("idle");
            sende("refresh");
          }, Math.min(2400, Math.max(700, took)));
        }
      },

      async copy_text({ text }) {
        try {
          await navigator.clipboard.writeText(String(text));
        } catch (_) {
          /* Vorschau ohne Zugriff auf die Zwischenablage */
        }
      },

      async delete_history({ id }) {
        z.history = z.history.filter((e) => e.id !== id);
        return kopie();
      },

      async clear_history() {
        z.history = [];
        return kopie();
      },

      async open_models_dir() {
        console.info("[Vorschau] Modellordner:", z.models_dir);
      },

      async download_model({ id }) {
        const k = z.catalog.find((x) => x.id === id);
        if (!k) throw "Dieses Modell kennt FreeSpeech nicht.";
        if (z.download && z.download.state === "running") {
          throw "Es lädt schon ein Modell. Warte, bis es fertig ist, oder brich es ab.";
        }
        const SCHRITTE = 12;                         // alle 500 ms einer, zusammen rund 6 Sekunden
        let schritt = 0;
        const stand = (state, message) => {
          z.download = {
            id,
            done_mb: Math.round((k.size_mb * schritt) / SCHRITTE),
            total_mb: k.size_mb,
            state,
            message: message || "",
          };
          return { ...z.download };
        };
        clearInterval(downloadTimer);
        stand("running");
        downloadTimer = setInterval(() => {
          schritt += 1;
          if (fehlerOffen && schritt === 5) {
            fehlerOffen = false;
            clearInterval(downloadTimer);
            sende("download", stand("error", "Der Download ist fehlgeschlagen. Prüfe die Internetverbindung und versuch es nochmal."));
            return;
          }
          if (schritt < SCHRITTE) {
            sende("download", stand("running"));
            return;
          }
          clearInterval(downloadTimer);
          const fertig = stand("done");
          z.download = null;
          const ziel = KATALOG_DATEI[id];
          const pfad = `${ORDNER}\\${ziel.file}`;
          if (!z.models.some((m) => m.file === ziel.file)) {
            z.models.push({ path: pfad, name: ziel.name, file: ziel.file, size_mb: k.size_mb, source: "Modellordner" });
          }
          sende("download", fertig);
          if (!z.engine) {                           // noch keines aktiv: gleich auswählen und laden
            z.settings.model = pfad;
            ladeModell();
          }
          sende("refresh");
        }, 500);
        return kopie();
      },

      async cancel_download() {
        clearInterval(downloadTimer);
        const d = z.download;
        z.download = null;                           // räumt auch einen stehengebliebenen Fehler weg
        if (d && d.state === "running") sende("download", { ...d, state: "cancelled", message: "" });
        return kopie();
      },

      async quit_app() {
        console.info("[Vorschau] FreeSpeech beenden");
      },
    };

    return {
      invoke(name, args) {
        if (!befehle[name]) return Promise.reject(`Unbekannter Befehl: ${name}`);
        return befehle[name](args || {});
      },
      listen(name, fn) {
        if (!hoerer.has(name)) hoerer.set(name, []);
        hoerer.get(name).push(fn);
        return Promise.resolve(() => {
          const liste = hoerer.get(name) || [];
          const i = liste.indexOf(fn);
          if (i >= 0) liste.splice(i, 1);
        });
      },
    };
  }
})();
