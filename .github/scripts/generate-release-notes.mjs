#!/usr/bin/env node
import { execFileSync } from "node:child_process";
import { appendFileSync, writeFileSync } from "node:fs";

const tag = process.argv[2] || process.env.RELEASE_TAG || process.env.GITHUB_REF_NAME;
const notesPath = process.argv[3] || "release-notes.md";

if (!tag) {
  console.error("Usage: generate-release-notes.mjs <tag> [notes-path]");
  process.exit(1);
}

function run(command, args, options = {}) {
  try {
    return execFileSync(command, args, {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
      ...options,
    }).trim();
  } catch {
    return "";
  }
}

function parseSemver(value) {
  const match = /^v?(\d+)\.(\d+)\.(\d+)(?:[-+].*)?$/.exec(value);
  if (!match) return null;
  return {
    major: Number(match[1]),
    minor: Number(match[2]),
    patch: Number(match[3]),
  };
}

function compareSemver(a, b) {
  return a.major - b.major || a.minor - b.minor || a.patch - b.patch;
}

function previousReleaseTag(currentTag) {
  const current = parseSemver(currentTag);
  if (!current) return "";

  const tags = run("git", ["tag", "--list", "v*"])
    .split(/\r?\n/)
    .map((name) => ({ name, version: parseSemver(name) }))
    .filter((entry) => entry.version && compareSemver(entry.version, current) < 0)
    .sort((a, b) => compareSemver(b.version, a.version));

  return tags[0]?.name || "";
}

function parseCommits(range) {
  const raw = run("git", [
    "log",
    "--first-parent",
    "--reverse",
    "--format=%H%x1f%s%x1f%b%x1e",
    range,
  ]);
  if (!raw) return [];

  return raw
    .split("\x1e")
    .map((record) => record.trim())
    .filter(Boolean)
    .map((record) => {
      const [sha, subject, ...bodyParts] = record.split("\x1f");
      return {
        sha,
        subject: subject.trim(),
        body: bodyParts.join("\x1f").trim(),
      };
    });
}

function issueRefs(text) {
  const refs = new Set();
  for (const match of text.matchAll(/\b(?:Ref|See)\s+#(\d+)\b/gi)) {
    refs.add(match[1]);
  }
  return [...refs];
}

function prNumberFromSubject(subject) {
  return (
    /^Merge pull request #(\d+)/.exec(subject)?.[1] ||
    /^Merge PR #(\d+)/.exec(subject)?.[1] ||
    /\(#(\d+)\)$/.exec(subject)?.[1] ||
    ""
  );
}

function isNoiseSubject(subject) {
  return (
    /^Bump version to\b/i.test(subject) ||
    /^Merge origin\/main\b/i.test(subject)
  );
}

function stripGeneratedBlocks(body) {
  return body.replace(/<!-- This is an auto-generated comment:[\s\S]*?<!-- end of auto-generated comment:[\s\S]*?-->/g, "");
}

function summaryBullets(body) {
  const clean = stripGeneratedBlocks(body);
  const summary = /## Summary\s+([\s\S]*?)(?:\n##\s+|$)/i.exec(clean)?.[1] || "";
  return summary
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => /^[-*]\s+/.test(line))
    .map((line) => line.replace(/^[-*]\s+/, "").replace(/\.$/, ""))
    .slice(0, 2);
}

function mergedPullRequests(previousTag, currentTag) {
  const repo = process.env.GITHUB_REPOSITORY || "kristofferR/Carrier";
  const previousDate = previousTag ? Date.parse(run("git", ["log", "-1", "--format=%aI", previousTag])) : 0;
  const currentDate = Date.parse(run("git", ["log", "-1", "--format=%aI", currentTag]));
  const json = run("gh", [
    "pr",
    "list",
    "--repo",
    repo,
    "--state",
    "merged",
    "--base",
    "main",
    "--limit",
    "100",
    "--json",
    "number,title,body,url,mergedAt",
  ]);
  if (!json) return null;

  try {
    return JSON.parse(json)
      .filter((pr) => {
        const mergedAt = Date.parse(pr.mergedAt);
        return mergedAt > previousDate && mergedAt <= currentDate;
      })
      .sort((a, b) => Date.parse(a.mergedAt) - Date.parse(b.mergedAt));
  } catch {
    return null;
  }
}

function cleanTitle(subject) {
  return subject
    .replace(/^Merge pull request #\d+ from \S+\s*/i, "")
    .replace(/^Merge PR #\d+:\s*/i, "")
    .replace(/\s+\(#\d+\)$/i, "")
    .replace(/^(fix|add|improve|extend|gate|use|keep)\s+/i, "")
    .replace(/\s+/g, " ")
    .trim();
}

function sentenceFromTitle(title) {
  const cleaned = cleanTitle(title);
  if (/^macOS\b/.test(cleaned)) return cleaned;
  return `${cleaned.charAt(0).toUpperCase()}${cleaned.slice(1)}`;
}

function classify(entry) {
  if (entry.section) return entry.section;
  const text = `${entry.title} ${entry.summary.join(" ")}`.toLowerCase();
  if (/\bfix|bug|release publish|ci\b/.test(text)) {
    return "fixes";
  }
  if (/\badd|setting|new|privacy|hide names|avatar|emoji\b/.test(text)) {
    return "new";
  }
  return "improvements";
}

function topic(entry) {
  const text = `${entry.title} ${entry.summary.join(" ")}`.toLowerCase();
  if (/hide names|privacy|avatar|identity/.test(text)) return "Privacy";
  if (/download|filename|media/.test(text)) return "Downloads";
  if (/dock|tray|reopen|wayland|linux|dmabuf|desktop/.test(text)) return "Desktop Polish";
  if (/notification|unread|badge/.test(text)) return "Notifications";
  if (/settings|emoji/.test(text)) return "Settings";
  if (/release|ci|workflow|readme/.test(text)) return "Release Polish";
  return "";
}

function displayHeading(entry) {
  const text = entry.title.toLowerCase();
  if (/hide names|privacy/.test(text)) return "Stronger Hide Names & Avatars";
  if (/system emoji|emoji/.test(text)) return "System emoji setting";
  return sentenceFromTitle(entry.title);
}

function displayCopy(entry) {
  const text = entry.title.toLowerCase();
  if (/system emoji|emoji/.test(text)) {
    return "Prefer your platform's native emoji rendering instead of Messenger's emoji style from Settings.";
  }
  if (/hide names|privacy/.test(text)) {
    return "Privacy mode now covers more Messenger identity surfaces while keeping message text readable.";
  }
  if (/media.*download|download.*filename|filename.*download/.test(text)) {
    return "Messenger image and video saves now default to readable filenames, preserve the right extension, and avoid clobbering existing files with numbered names.";
  }
  if (/dock and tray|dock reopen|tray unread|reopen/.test(text)) {
    return "Reopening Carrier from the Dock returns to the main Messenger window more reliably, tray/window behavior is tighter, and unread tray text clears when the count hits zero.";
  }
  if (/wayland|dmabuf/.test(text)) {
    return "Carrier applies the WebKit DMABUF workaround on affected Wayland sessions unless you have explicitly supplied your own override.";
  }
  if (/release publishing|release polish/.test(text)) {
    return "Draft release cleanup, README download links and platform-specific CI helpers are handled without blocking the publish step.";
  }
  if (entry.summary.length > 0) {
    const [first, second] = entry.summary;
    return `${first}${second ? `, and ${second.charAt(0).toLowerCase()}${second.slice(1)}` : ""}.`;
  }
  return `${sentenceFromTitle(entry.title)}.`;
}

function suffix(entry) {
  const refs = [];
  if (entry.pr) refs.push(`#${entry.pr}`);
  for (const ref of entry.refs) {
    if (!refs.includes(`#${ref}`)) refs.push(`#${ref}`);
  }
  return refs.length ? ` (${refs.join(", ")})` : "";
}

function output(name, value) {
  if (!process.env.GITHUB_OUTPUT) return;
  const delimiter = `EOF_${name}_${Date.now()}`;
  appendFileSync(process.env.GITHUB_OUTPUT, `${name}<<${delimiter}\n${value}\n${delimiter}\n`);
}

const version = tag.replace(/^v/, "");
const lineVersion = parseSemver(tag);
const lineName = lineVersion ? `${lineVersion.major}.${lineVersion.minor}` : version;
const previousTag = previousReleaseTag(tag);
const range = previousTag ? `${previousTag}..${tag}` : tag;
const commits = parseCommits(range).filter((commit) => !isNoiseSubject(commit.subject));
const entries = [];
const seenPrs = new Set();

for (const pr of mergedPullRequests(previousTag, tag) || []) {
  const number = String(pr.number);
  seenPrs.add(number);
  entries.push({
    title: pr.title,
    pr: number,
    refs: [],
    summary: summaryBullets(pr.body || ""),
  });
}

for (const commit of commits) {
  const pr = prNumberFromSubject(commit.subject);
  if (pr) {
    if (seenPrs.has(pr)) continue;
    seenPrs.add(pr);
    entries.push({
      title: cleanTitle(commit.subject),
      pr,
      refs: [],
      summary: summaryBullets(commit.body),
    });
    continue;
  }

  const subjectRefs = issueRefs(`${commit.subject}\n${commit.body}`);
  const coveredByPr = entries.some((entry) => {
    const text = `${entry.title} ${entry.summary.join(" ")}`.toLowerCase();
    const subject = commit.subject.toLowerCase();
    return (
      (subjectRefs.length && subjectRefs.some((ref) => entry.refs.includes(ref))) ||
      (/(dock|tray|reopen|unread)/.test(subject) && /(dock|tray|reopen|unread)/.test(text)) ||
      (/(download|filename)/.test(subject) && /(download|filename)/.test(text))
    );
  });
  if (coveredByPr) continue;

  entries.push({
    title: commit.subject,
    pr: "",
    refs: subjectRefs,
    summary: summaryBullets(commit.body),
  });
}

function combineEntries(items, predicate, combined) {
  const matched = items.filter(predicate);
  if (!matched.length) return items;
  const unmatched = items.filter((entry) => !predicate(entry));
  const refs = [...new Set(matched.flatMap((entry) => [...entry.refs, ...(entry.pr ? [entry.pr] : [])]))];
  unmatched.push({
    ...combined,
    pr: "",
    refs,
    summary: [],
  });
  return unmatched;
}

let releaseEntries = entries;
releaseEntries = combineEntries(
  releaseEntries,
  (entry) => /dock|tray|reopen|unread/i.test(`${entry.title} ${entry.summary.join(" ")}`),
  {
    title: "macOS Dock and tray behavior",
    section: "improvements",
  },
);
releaseEntries = combineEntries(
  releaseEntries,
  (entry) => /release|workflow|readme|non-macos ci|push_macos_webview_store_paths|\.sig/i.test(`${entry.title} ${entry.summary.join(" ")}`),
  {
    title: "Release publishing",
    section: "fixes",
  },
);

const sections = {
  new: [],
  improvements: [],
  fixes: [],
};

for (const entry of releaseEntries) {
  sections[classify(entry)].push(entry);
}

const topicPriority = ["Privacy", "Downloads", "Desktop Polish", "Notifications", "Settings", "Release Polish"];
const topics = [...new Set(releaseEntries.map(topic).filter(Boolean))]
  .sort((a, b) => topicPriority.indexOf(a) - topicPriority.indexOf(b))
  .filter((item, index, all) => item !== "Release Polish" || all.length === 1)
  .slice(0, 3);
const headline = topics.length ? topics.join(", ").replace(/, ([^,]*)$/, " & $1") : "Desktop Polish";
const titleEmoji = topics.includes("Privacy") ? " 🕶️" : "";
const releaseTitle = `Carrier ${version} — ${headline}${titleEmoji}`;
const highlightText = topics.length
  ? `${topics.join(", ").replace(/, ([^,]*)$/, " and $1").toLowerCase()}`
  : "desktop fixes and polish";

const lines = [
  `Carrier ${version} is a focused polish release for the ${lineName} line. This update brings ${highlightText}, with the same signed macOS, Windows and Linux downloads as usual.`,
  "",
];

if (sections.new.length) {
  lines.push("## What's New", "");
  for (const entry of sections.new) {
    lines.push(`### ${displayHeading(entry)}`, "");
    lines.push(`${displayCopy(entry)}${suffix(entry)}`, "");
  }
}

if (sections.improvements.length) {
  lines.push("## Improvements", "");
  for (const entry of sections.improvements) {
    lines.push(`- **${displayHeading(entry)}.** ${displayCopy(entry)}${suffix(entry)}`);
  }
  lines.push("");
}

if (sections.fixes.length) {
  lines.push("## Bug Fixes", "");
  for (const entry of sections.fixes) {
    lines.push(`- **${displayHeading(entry)}.** ${displayCopy(entry)}${suffix(entry)}`);
  }
  lines.push("");
}

lines.push("---", "");
lines.push("**Thanks** for using Carrier! Hit a bug or want a feature? [Open an issue](https://github.com/kristofferR/Carrier/issues). 🙂");

const releaseBody = lines.join("\n");
writeFileSync(notesPath, `${releaseBody}\n`);
output("release_title", releaseTitle);
output("release_body", releaseBody);

console.log(releaseTitle);
console.log(`Wrote ${notesPath}${previousTag ? ` from ${previousTag}..${tag}` : ""}`);
