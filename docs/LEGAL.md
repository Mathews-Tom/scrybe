# scrybe — Legal Posture

> Project posture, jurisdiction matrix, and intended-use statement for an open-source meeting-transcription tool that runs entirely on the user's own hardware.

This document is **not** legal advice. It is a written statement of how the project is positioned and why. The author is not your lawyer, and your jurisdiction's facts may differ. Where this document references statutes or cases, primary sources are linked.

---

## 1. Project posture

scrybe is **published software**, not a service. The author writes Rust source code and publishes binaries. Users download the binary, run it on their own hardware, and decide what audio it captures and what they do with the output. Nothing flows back to the author.

| Role | scrybe author | scrybe user |
|---|---|---|
| Determines what is recorded | No | Yes |
| Operates the recording device | No | Yes |
| Stores recorded data | No | Yes |
| Processes recorded data | No | Yes |
| Receives recorded data | No | No (fully local unless user configures cloud STT/LLM) |
| Markets to a regulated industry | No | (their choice) |

Under [GDPR Art. 4](https://gdpr-info.eu/art-4-gdpr/), the entity that "determines the purposes and means of the processing" is the controller. The author is **neither controller nor processor** — the user who runs the binary on their machine and decides what to feed it is the controller. This is the consensus reading and matches the [Article 29 Working Party](https://ec.europa.eu/justice/article-29/documentation/index_en.htm) guidance on software vendors.

Under HIPAA, [HHS FAQ 256](https://www.hhs.gov/hipaa/for-professionals/faq/256/is-software-vendor-business-associate/index.html) is explicit: "the mere selling or providing of software to a covered entity does not give rise to a business associate relationship." A clinician's later use of scrybe does not create author BAA exposure.

Under the EU AI Act, [Recital 102](https://artificialintelligenceact.eu/recital/102/) and [Article 2(12)](https://artificialintelligenceact.eu/article/2/) carve out "AI systems released under free and open-source licences" from most provider obligations, conditional on weights/architecture/usage info being publicly available. scrybe under Apache-2.0, free, no managed service, falls inside the exemption.

## 2. License

scrybe is licensed under **Apache License 2.0** (`LICENSE` in the repo root, full text at <https://www.apache.org/licenses/LICENSE-2.0>).

Apache-2.0 was chosen over MIT and AGPL for four concrete reasons:

1. **§3 explicit patent grant.** Every contributor grants a perpetual, worldwide, royalty-free patent license covering their contribution. MIT contains no patent grant; in patent-litigious areas (audio codecs, Whisper-related ML, on-device inference) this matters. The §3 termination clause also bites back: anyone who initiates patent litigation against the project loses their license — a defensive moat MIT lacks.
2. **§4 attribution-preserving redistribution.** Derivative works must "retain… all copyright, patent, trademark, and attribution notices" and must "carry prominent notices stating that You changed the files." This is a stronger anti-strip-mining provision than MIT's "include the notice somewhere." Forks of scrybe that delete attribution are in violation; scrubbing the project name from a fork is contractually impermissible.
3. **§6 trademark non-grant.** No license is granted to use the project name or marks except as required for §4 attribution. A fork called `scrybe-cloud` selling a SaaS is a §6 violation; a fork called `notequil-cloud` is fine. This protects the project's identity from being co-opted.
4. **§7 warranty disclaimer + §8 limitation of liability.** Both clauses are enforceable as copyright conditions per *[Jacobsen v. Katzer](https://en.wikipedia.org/wiki/Jacobsen_v._Katzer)*, 535 F.3d 1373 (Fed. Cir. 2008). Materially stronger drafted than MIT's single-sentence "AS IS" — Apache-2.0 §7 lists specific disclaimed warranties (title, non-infringement, merchantability, fitness for purpose) and §8 caps liability for "direct, indirect, special, incidental, or consequential damages" arising from use.

**Do not modify the disclaimer text in `LICENSE`.** The verbatim Apache-2.0 boilerplate is the asset; courts respect it because it's unmodified OSI-approved standard text. Custom edits weaken the enforceability story.

The §7 and §8 disclaimers bind users (who accept the license by using the software). They do **not** bind non-user recorded parties — a person recorded without consent has no contractual relationship with the author, and the disclaimer does not extinguish their statutory wiretap claims against the **user**. That risk lives entirely with the user, which is why §3 below exists.

### 2.1 Why not MIT

MIT was scrybe's initial pitch. It was wrong for three reasons: no patent grant, weaker attribution requirement, no trademark protection. Apache-2.0 strictly dominates MIT for a project of this risk profile (audio capture, ML inference, regulated-industry-adjacent users) at the cost of a slightly longer license file.

### 2.2 Why not AGPL

AGPL would protect against scrybe being forked into a closed-source SaaS — the precise pattern named in `pitch.md` as the failed-OSS template. It was rejected because:

- The "SaaS fork" risk is hypothetical; the "deters corporate OSS contributors" risk is real and immediate. AGPL contributors are rare in the Rust ecosystem outside specific niches (databases, certain CLI tools).
- The architectural choice to ship only a binary already prevents the obvious capture path. A SaaS fork would have to either rewrite the audio-capture stack or wrap the binary — both meaningful work that signals genuine differentiation rather than strip-mining.
- AGPL §13 (network-use clause) is famously hard to enforce against single-tenant deployments; its deterrent value is more aesthetic than legal.

## 3. Recording-consent law (user-facing)

Recording laws are jurisdiction-dependent. Users are responsible for compliance in their own jurisdiction and the jurisdiction of every party they record. scrybe ships consent UX (`--consent {quick,notify,announce}`, system-design §5) to make compliance practicable; choosing the right mode is the user's call.

### 3.1 United States — federal

[18 U.S.C. § 2510–2522](https://www.law.cornell.edu/uscode/text/18/2511) (the Wiretap Act / ECPA) requires at least **one-party consent** for the interception of a wire, oral, or electronic communication. Under federal law, recording a conversation you are a participant in is generally lawful.

### 3.2 United States — state two-party / all-party consent

The following states require **all parties** to consent. State law applies based on the location of the party being recorded; in interstate calls, the strictest rule typically governs.

| State | Statute | Notes |
|---|---|---|
| California | [Penal Code § 632](https://leginfo.legislature.ca.gov/faces/codes_displaySection.xhtml?lawCode=PEN&sectionNum=632) | "Confidential communication" standard. Up to $2,500 / 1 year per violation; private right of action |
| Connecticut | [Gen. Stat. § 52-570d](https://www.cga.ct.gov/current/pub/chap_899.htm) | Civil only |
| Delaware | [11 Del. C. § 1335](https://delcode.delaware.gov/title11/c005/sc07/index.html) | Civil and criminal |
| Florida | [Fla. Stat. § 934.03](https://www.flsenate.gov/Laws/Statutes/2024/934.03) | Felony |
| Illinois | [720 ILCS 5/14-2](https://www.ilga.gov/legislation/ilcs/fulltext.asp?DocName=072000050K14-2); BIPA [740 ILCS 14](https://www.ilga.gov/legislation/ilcs/ilcs3.asp?ActID=3004) | Strong private right of action; *Cruz v. Fireflies.AI* litigates BIPA application to AI transcripts |
| Maryland | [Md. Code § 10-402](https://mgaleg.maryland.gov/mgawebsite/Laws/StatuteText?article=gcj&section=10-402) | Felony |
| Massachusetts | [Mass. Gen. Laws c. 272 § 99](https://malegislature.gov/Laws/GeneralLaws/PartIV/TitleI/Chapter272/Section99) | Felony; recording requires *secrecy* not just non-consent |
| Montana | [Mont. Code § 45-8-213](https://leg.mt.gov/bills/mca/title_0450/chapter_0080/part_0020/section_0130/0450-0080-0020-0130.html) | Misdemeanor |
| Nevada | [NRS 200.620](https://www.leg.state.nv.us/NRS/NRS-200.html#NRS200Sec620) | Plus federal-style rules |
| New Hampshire | [RSA 570-A:2](http://www.gencourt.state.nh.us/rsa/html/lviii/570-a/570-a-2.htm) | Felony |
| Pennsylvania | [18 Pa. C.S. § 5703](https://www.legis.state.pa.us/cfdocs/legis/LI/consCheck.cfm?txtType=HTM&ttl=18&div=00&chpt=57&sctn=03&subsctn=000) | Felony |
| Washington | [RCW 9.73.030](https://app.leg.wa.gov/RCW/default.aspx?cite=9.73.030) | Misdemeanor or felony |

EFF maintains a maintained tracker: <https://www.eff.org/issues/recording-laws>.

Recommended `--consent` mode in two-party-consent jurisdictions: `announce` (in-call spoken disclosure + chat-message paste).

### 3.3 European Union, United Kingdom, Switzerland

GDPR Article 6 lawful-basis options most relevant:

- **Art. 6(1)(a) explicit consent** — preferred. Requires affirmative, documented opt-in before recording.
- **Art. 6(1)(f) legitimate interests** — only after a documented balancing test; rarely sufficient for meeting recordings.

The UK ICO ([Personal information that's recorded](https://ico.org.uk/for-the-public/personal-information/)) treats meeting transcripts as personal data; the same Article 6 analysis applies.

Switzerland (FADP) and the GDPR-equivalent regimes in Norway, Iceland, and Liechtenstein follow the same pattern.

Recommended `--consent` mode: `announce` or `notify` with explicit chat acknowledgement before recording starts.

### 3.4 Canada

Federal one-party consent under [Criminal Code § 184](https://laws-lois.justice.gc.ca/eng/acts/c-46/section-184.html). Provincial privacy statutes (BC, AB, QC, MB, SK) impose additional obligations on personal-information processing. Recommended: `notify` minimum, `announce` for QC and BC.

### 3.5 Australia

Telecommunications Interception and Access Act + state Surveillance Devices Acts. Most states require all-party consent; rules vary. Recommended: `announce`.

### 3.6 India, Singapore, Japan, Brazil, Mexico

One-party-consent or telecom-act regimes. LGPD (Brazil) and APPI (Japan) impose data-handling obligations on the user but do not change the consent rule. `notify` is generally sufficient; `announce` for clarity in disputed contexts.

## 4. Author exposure (publish-only posture)

This section addresses the question: *"What's the legal exposure of an OSS author who publishes a Rust binary that some downstream user later misuses to record someone illegally?"*

### 4.1 US wiretap statutes

[18 U.S.C. § 2511](https://www.law.cornell.edu/uscode/text/18/2511) reaches anyone who "procures any other person to intercept" — i.e., aiding-and-abetting and procurement liability. The Supreme Court's binding standard for inducement liability against a software publisher is *[MGM Studios v. Grokster](https://supreme.justia.com/cases/federal/us/545/913/)*, 545 U.S. 913 (2005): liability attaches only on "purposeful, culpable expression and conduct… clear expression or other affirmative steps taken to foster infringement." A neutrally-marketed dual-use tool with substantial non-infringing uses is generally protected.

scrybe's posture is squarely within the protected zone:

- **Substantial non-infringing uses**: solo dictation, post-meeting note-taking with full participant consent, recording of public-record events, recording in one-party-consent jurisdictions where the user is a party.
- **Neutral marketing**: README and pitch describe consent as a first-class feature; nothing markets the tool for stealth recording.
- **Affirmative anti-misuse steps**: mandatory consent step (`--consent` cannot be disabled at compile or runtime; floor is `quick`); jurisdiction-aware UX modes; this document.

### 4.2 §230 and DMCA §512

Neither shields the **author** of code. Section 230 protects platforms hosting user content; §512 protects service providers receiving copyright takedowns. Both are platform shields. They are mentioned here only to dispel confusion: GitHub and crates.io may benefit from these statutes; the author does not, and does not need to.

### 4.3 GDPR

Per Article 4, the controller is the entity that "determines the purposes and means of the processing." The author who runs no servers and receives no telemetry is **neither controller nor processor**. The user who runs the binary and decides what audio to capture is the controller. This is settled doctrine for software vendors; the author has no GDPR obligations beyond the user's GDPR obligations to participants they record.

### 4.4 HIPAA

[HHS FAQ 256](https://www.hhs.gov/hipaa/for-professionals/faq/256/is-software-vendor-business-associate/index.html) is dispositive: software publishers who do not "create, receive, maintain, or transmit" PHI on behalf of a covered entity are not business associates and do not require BAAs. A clinician installing scrybe and running it on their own machine creates no BAA exposure for the author.

### 4.5 EU AI Act

[Recital 102](https://artificialintelligenceact.eu/recital/102/) and [Article 2(12)](https://artificialintelligenceact.eu/article/2/) carve out free and open-source AI systems from most deployer/provider obligations. scrybe satisfies the conditions: weights and source available, no payment for the binary, no managed cloud offering. Carve-out applies until and unless scrybe (a) starts being placed on the market against payment, (b) becomes a GPAI model with systemic risk, or (c) becomes a high-risk Annex III system.

### 4.6 Products liability

US products-liability doctrine applied to software is unsettled. Courts generally treat software as licensed work, not a strict-liability product. Apache-2.0 §7 (warranty disclaimer) and §8 (limitation of liability) are enforceable contract terms (see *Jacobsen v. Katzer*) and reduce author exposure to user-side claims to near zero. They do not bind non-user recorded parties; that risk sits with the user.

### 4.7 Precedent — published cases of OSS author liability for recording-adjacent tools

We searched for cases 2020–2026 in which an individual open-source author was sued or criminally charged purely for publishing a recording, eavesdropping, or wiretap-adjacent tool. **We found none.**

Closest reference points:

- **Mimikatz / Benjamin Delpy** — security tool with 13+ years of hostile use by both red teams and attackers. No civil suit, no criminal charge. Cleanest analog for security-adjacent OSS publication.
- **youtube-dl / yt-dlp** — RIAA DMCA takedown reversed by GitHub; no individual author was sued.
- **DeCSS / Jon Johansen** — injunction issued against publishers and linkers; original author Johansen was acquitted in Norway (2003). DMCA-era and not directly on-point.
- **Tornado Cash / Roman Storm** — convicted on operating an unlicensed money-transmitting business; jury hung on money-laundering and sanctions counts. *Operator-adjacent*, not pure publication. *Van Loon v. Treasury* (5th Cir. 2024) held that immutable smart contracts are "just code software" outside OFAC authority — supportive of the publish-only posture.

The author's exposure is materially low. The mitigations below are belt-and-braces, not necessity.

## 5. Mitigations the project ships

### 5.1 Consent UX (system-design §5)

`--consent {quick,notify,announce}` is mandatory. It cannot be disabled at compile time or runtime. `ConsentAttestation` is logged into `meta.toml` with mode, timestamp, and operating user — providing the user evidence of good-faith compliance if a dispute arises.

### 5.2 Intended-use statement (in `README.md`)

> scrybe is a local recording tool intended for use where the user has obtained all consents required by the laws of their jurisdiction and the jurisdictions of the parties they record. Use of scrybe in violation of applicable wiretap, eavesdropping, surveillance, or data-protection laws is prohibited. The author publishes scrybe under the Apache License 2.0 with no warranty (§7) and limited liability (§8); users are responsible for their own compliance. See `docs/LEGAL.md`.

### 5.3 Neutral marketing

No language in any project artifact (README, pitch, blog posts, social media) markets scrybe for secret, stealth, covert, surreptitious, or undisclosed recording. This is non-negotiable. PRs that introduce such language should be rejected on legal grounds, not just style grounds.

### 5.4 No managed service, no SaaS, no hosted endpoint

The architectural decision to ship only a binary is also a legal decision. Operating a service crosses a line that publishing software does not. *U.S. v. Storm* (Tornado Cash) demonstrates that the operator/publisher distinction matters: the conviction was on operating-without-license, not on publishing the contracts. scrybe stays on the publish-only side of that line.

### 5.5 Jurisdiction notice in `README.md`

A short block in the README points users at the EFF state-by-state recording-laws tracker and names the all-party-consent jurisdictions explicitly. Affirmative warning weighs against any *Grokster*-style "fostering infringement" theory.

## 6. Disclaimers

This document is not legal advice. The author is not a lawyer, and nothing here should be relied upon as a substitute for advice from qualified counsel familiar with your jurisdiction and facts. Statutes, case law, and regulatory guidance change; the citations here reflect the state of the law as of 2026-04. If you operate in a regulated industry (healthcare, legal, finance, defense) or in an all-party-consent jurisdiction, consult counsel before deploying scrybe in production.

If you are a recorded participant who believes you have been recorded without your consent in violation of applicable law, your dispute is with the **user** of scrybe, not with the author. The author runs no servers, has no access to any recording, and has no record of your conversation. The user is the data controller; the user is the recording party; the user is the legally responsible party.

## 7. Reporting concerns

Suspected misuse of scrybe (e.g., a project, fork, or distribution that markets the tool for unlawful purposes) can be reported to the maintainer via the address in `SECURITY.md`. The maintainer's response will be limited to the project itself: license enforcement, takedown of misleading downstream marketing, public statement of intended-use posture. The maintainer is not in a position to investigate or respond to complaints about specific recordings made by specific users.

## 8. References

- [18 U.S.C. § 2511](https://www.law.cornell.edu/uscode/text/18/2511) — Federal Wiretap Act
- [Cal. Penal Code § 632](https://leginfo.legislature.ca.gov/faces/codes_displaySection.xhtml?lawCode=PEN&sectionNum=632) — California eavesdropping statute
- [GDPR Art. 4](https://gdpr-info.eu/art-4-gdpr/) — Definitions (controller, processor)
- [HHS FAQ 256](https://www.hhs.gov/hipaa/for-professionals/faq/256/is-software-vendor-business-associate/index.html) — Software vendor vs. business associate
- [EU AI Act Recital 102](https://artificialintelligenceact.eu/recital/102/) — FOSS carve-out
- [MGM v. Grokster, 545 U.S. 913 (2005)](https://supreme.justia.com/cases/federal/us/545/913/) — Inducement standard
- [Jacobsen v. Katzer, 535 F.3d 1373 (Fed. Cir. 2008)](https://en.wikipedia.org/wiki/Jacobsen_v._Katzer) — OSS license enforceability
- [Bernstein v. US Dept. of Justice (9th Cir. 1999)](https://www.eff.org/cases/bernstein-v-us-dept-justice) — Source code as speech
- [Brewer v. Otter.ai (N.D. Cal. 2025)](https://www.fisherphillips.com/a/web/x27EBgcvus2uFdfXMJiyCk/aAQ5CP/brewer-v-otterai.pdf) — ECPA/CIPA class action
- [Cruz v. Fireflies.AI (IL 2025)](https://www.workplaceprivacyreport.com/2026/04/articles/artificial-intelligence/ai-meeting-assistants-and-biometric-privacy-governance-lessons-from-the-fireflies-ai-lawsuit/) — BIPA litigation
- [U.S. v. Storm (S.D.N.Y. 2025)](https://www.mayerbrown.com/en/insights/publications/2025/08/the-tornado-cash-trials-mixed-verdict-implications-for-developer-liability) — Operator vs. publisher distinction
- [EFF: State recording laws](https://www.eff.org/issues/recording-laws) — Maintained tracker
