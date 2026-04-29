# scrybe — Legal Posture

> Reference card for the project's licensing, distribution model, and user-facing recording-rules matrix. **Not legal advice.** Consult counsel if your situation calls for it.

scrybe is **published software**, not a service. The author writes Rust source, publishes binaries, and runs no servers. Users download the binary, run it on their own hardware, and decide what audio it captures and what they do with the output. Nothing flows back to the author.

---

## 1. Project posture at a glance

| Role | Author of scrybe | User of scrybe |
|---|---|---|
| Determines what is captured | No | Yes |
| Operates the device | No | Yes |
| Stores any captured data | No | Yes |
| Receives any captured data | No | No (fully local unless the user configures a cloud STT/LLM endpoint) |
| Markets to a specific industry | No | (their choice) |
| Operates a managed service | No | (n/a) |

The author is neither a data controller nor a data processor under GDPR Article 4 (the user, who runs the binary, is the controller). The author is not a HIPAA business associate per HHS FAQ 256 ("the mere selling or providing of software… does not give rise to a business associate relationship"). scrybe falls inside the EU AI Act's free-and-open-source carve-out (Recital 102, Article 2(12)) so long as no managed service is offered against payment.

## 2. License — Apache-2.0

scrybe is licensed under **Apache License 2.0**. Full text: <https://www.apache.org/licenses/LICENSE-2.0>. Repo file: `LICENSE`.

Why Apache-2.0 (and not MIT, not AGPL):

| Apache clause | What it gives the project |
|---|---|
| §3 patent grant | Each contributor grants a perpetual, worldwide, royalty-free patent license. Termination clause: anyone who initiates patent litigation against the project loses their license. MIT has no patent grant |
| §4 attribution-preserving redistribution | Derivatives must "retain… all copyright, patent, trademark, and attribution notices" and "carry prominent notices stating that You changed the files." Stronger than MIT's "include the notice somewhere" |
| §6 trademark non-grant | Forks may not use the project name to endorse derivative works. Protects identity from being co-opted |
| §7 warranty disclaimer + §8 limitation of liability | Specific disclaimed warranties (title, non-infringement, merchantability, fitness) and capped liability. Enforceable as copyright conditions per *Jacobsen v. Katzer* |

Why not AGPL: the SaaS-fork risk is hypothetical; the corporate-OSS-contributor deterrence is real and immediate. The architectural choice to ship only a binary already prevents the obvious capture path.

**Do not modify the boilerplate text in `LICENSE`.** Verbatim Apache-2.0 is the legal asset; custom edits weaken enforceability.

## 3. User-facing recording rules

Recording rules vary by jurisdiction. Users are responsible for compliance in their own jurisdiction and the jurisdiction of every party they record. scrybe ships a courtesy-notification step (`--consent {quick,notify,announce}`, system-design §5) so users can pick the right level of disclosure for their context.

### 3.1 Mode selector

| Mode | What it does | When to use |
|---|---|---|
| `quick` | Local CLI prompt; records the user's acknowledgement to `meta.toml` | Solo dictation; user is the only participant |
| `notify` | Posts a one-line courtesy message into the meeting chat at start | Multi-party calls; courtesy-notice norm |
| `announce` | Adds a 2-second spoken disclosure played via the user's mic at session start | When a stronger explicit notice is appropriate |

The step cannot be disabled; `quick` is the floor.

### 3.2 Reference matrix

Pick the mode that matches the strictest rule across all participants. EFF maintains a state-by-state US tracker at <https://www.eff.org/issues/recording-laws>.

| Region | Default rule | Suggested mode |
|---|---|---|
| US — federal | One-party (18 U.S.C. § 2511) | `notify` |
| US — CA, CT, DE, FL, IL, MD, MA, MT, NV, NH, PA, WA | All-party rule applies | `announce` |
| US — other states | One-party | `notify` |
| EU / UK / EEA | GDPR Art. 6 lawful basis required; explicit consent preferred | `announce` (or `notify` with chat acknowledgement) |
| Switzerland (FADP) | Similar to GDPR | `announce` |
| Canada — federal | One-party (Criminal Code § 184) | `notify` minimum |
| Canada — BC, AB, QC, MB, SK | Provincial privacy statutes add obligations | `announce` |
| Australia | State-by-state; many states require all-party | `announce` |
| India / Singapore / Japan / Brazil / Mexico | One-party or telecom-act regime | `notify` |

Statute citations live in §6 below; this section is operational guidance, not legal advice.

## 4. Publisher-posture summary

### 4.1 Why a software publisher who runs no service has a low-risk posture

| Concern | Status for scrybe's author |
|---|---|
| US wiretap statutes (federal + state) | Reaches "purposeful, culpable expression" to foster unlawful use (*MGM v. Grokster*). scrybe is neutrally marketed, has substantial non-infringing uses, and ships a courtesy-notification step as default. Does not meet the inducement bar |
| §230 / DMCA §512 | Both are platform shields, not author shields. Irrelevant to this analysis |
| GDPR | Author is not the controller. Software vendors are not controllers under Article 29 Working Party guidance |
| HIPAA | HHS FAQ 256: "mere selling or providing of software" does not create a business-associate relationship |
| EU AI Act | Recital 102 / Art. 2(12) free-and-open-source carve-out applies; condition is no managed service against payment |
| Products liability | Software is generally treated as licensed work, not a strict-liability product. Apache-2.0 §7–§8 disclaimers are enforceable per *Jacobsen v. Katzer* |
| Precedent — OSS authors sued for publishing recording-adjacent tools (2020–2026) | None found in published cases |

### 4.2 Project mitigations

- Courtesy-notification UX is mandatory; cannot be removed at compile or runtime.
- Intended-use statement in `README.md` (see §5 below).
- Neutral marketing — no language framing the tool for stealth use.
- No managed service, no hosted endpoint, no telemetry. Stays inside the AI Act FOSS carve-out.
- Apache-2.0 unmodified.

## 5. README intended-use statement

This sentence appears in `README.md`:

> scrybe is a meeting notetaker for the user's own meetings. Use it where you have all consents required by the laws of your jurisdiction and the jurisdictions of the parties you record. Use in violation of applicable wiretap, eavesdropping, surveillance, or data-protection laws is prohibited. The author publishes scrybe under the Apache License 2.0 with no warranty (§7) and limited liability (§8); users are responsible for their own compliance. See `docs/LEGAL.md`.

## 6. References

### Statutes / regulations

- US federal: [18 U.S.C. § 2511](https://www.law.cornell.edu/uscode/text/18/2511).
- California: [Penal Code § 632](https://leginfo.legislature.ca.gov/faces/codes_displaySection.xhtml?lawCode=PEN&sectionNum=632).
- GDPR: [Article 4](https://gdpr-info.eu/art-4-gdpr/), [Article 6](https://gdpr-info.eu/art-6-gdpr/).
- HIPAA: [HHS FAQ 256](https://www.hhs.gov/hipaa/for-professionals/faq/256/is-software-vendor-business-associate/index.html).
- EU AI Act: [Recital 102](https://artificialintelligenceact.eu/recital/102/), [Article 2](https://artificialintelligenceact.eu/article/2/).
- US state-by-state tracker: [EFF recording laws](https://www.eff.org/issues/recording-laws).

### Cases

- *Bernstein v. US Department of Justice* (9th Cir. 1999) — source code as First Amendment speech.
- *Universal v. Corley* (2d Cir. 2001) — code-as-speech, regulable for non-expressive function.
- *MGM Studios v. Grokster*, [545 U.S. 913 (2005)](https://supreme.justia.com/cases/federal/us/545/913/) — inducement standard.
- *Fair Housing Council v. Roommates.com*, 521 F.3d 1157 (9th Cir. 2008) — §230 limits.
- *Jacobsen v. Katzer*, [535 F.3d 1373 (Fed. Cir. 2008)](https://en.wikipedia.org/wiki/Jacobsen_v._Katzer) — OSS license enforceability.

## 7. Reporting concerns

If you believe a project, fork, or distribution is marketing scrybe for unlawful use, contact the maintainer via the address in `SECURITY.md`. The maintainer's response is limited to: license enforcement, takedown of misleading downstream marketing, public statement of intended-use posture. The maintainer cannot investigate or respond to complaints about specific captures made by specific users.

## 8. Disclaimer

This document is not legal advice. The author is not a lawyer. Statutes, regulations, and case law change. Consult counsel familiar with your jurisdiction and facts.
