# Using NEVO

NEVO is a library that lives on a small computer near you. It works without the internet. You open it in the web browser of any phone, tablet or laptop. There is nothing to install.

> **Guide version:** first edition, for NEVO 0.1 (October 2026). Parts marked **Coming soon** are planned but not built yet.

## Open NEVO

You need:

- A phone, tablet or laptop with a web browser
- The NEVO Wi-Fi name, its password (if it has one), and the NEVO address. Ask whoever looks after the node; it's often written on a card next to it.

Steps:

1. Open your Wi-Fi settings and join the NEVO network.
2. If your phone says the network has **no internet**, choose to **stay connected**. If pages don't load, turn off mobile data.
3. Open the web browser and type the address exactly, for example `http://10.42.0.1:8080`. Start with `http://`, and keep the `:8080` at the end if the address has one.
4. Bookmark the page, or add it to your home screen, so you don't have to type it again.

NEVO works on old phones, and with JavaScript turned off.

## Find your way around

Every page has three links at the top:

| Link | What it shows |
|---|---|
| **Library** | The front page: all topics, and the guides added most recently |
| **Topics** | Every topic and its smaller topics, such as **medical / first-aid** or **water / treatment** |
| **Add a guide** | The form to add your own guide |

Click **NEVO** at the top left to return to the front page from anywhere.

**Coming soon:** a search box, and an AI assistant you can ask questions such as "how do I treat a burn?". It will answer only from the library and show the guides it used.

## Browse topics

1. Click **Topics**.
2. Click a topic (such as **water**) to see everything in it, or a smaller topic (such as **treatment**) to narrow it down.
3. Click a guide's title to open it.

The topics are: medical, water, food, farming, energy, shelter, repair, safety, communication, education, maps, culture and general. Each has smaller topics.

A topic that shows "Nothing in this topic yet" is waiting for someone to add the first guide.

## Read a guide

A guide's page shows, from top to bottom:

1. **Its topics.** Click one to see similar guides.
2. **The title and summary.**
3. **Facts about it:**
   - "Added on this NEVO" or "From another NEVO"
   - the language
   - the license (what you may do with it; see "Licenses, simply" below)
   - "Only on this NEVO", if the author chose to keep it here
4. **The text,** if it has some.
5. **Files.** Photos are shown in the page. Videos and audio play in the page. PDFs open in the browser. Other files download when you click them.
6. **History:** each version, with its date and a short code for its author.

Warning: NEVO can't yet tell you **how much to trust** a guide. Anyone with access can add one, and nobody has checked it. For anything that could hurt someone (medicine, electricity, chemicals, food safety), compare with other sources and use your judgement.

**Coming soon:** trust labels (Reference, Verified, Community, Unknown), so you can see which guides were checked by people you trust.

## Add a guide

Warnings first:

- **Check everything before you press Add.** You can't edit or delete a guide yet (**coming soon**). Someone who looks after the node may be able to help later.
- **Photos can contain the place where they were taken.** Many phones store the GPS location inside each photo. NEVO doesn't remove it yet (**coming soon**). If the location is private, turn off location for the camera before taking the photo, or remove the location in your phone's photo app before uploading.
- **Only add what you made yourself, or what you have the right to share.**

You need:

- What you know, written as steps, and/or files: photos, PDFs, video
- A few minutes. Large videos take a while to upload over Wi-Fi.

Steps:

1. Click **Add a guide**.
2. **Title:** say what it helps with, plainly. For example: "Make water safe to drink with sunlight".
3. **Summary** (optional): one or two sentences saying what the guide covers.
4. **Topics:** pick at least one, and up to 16.
   - On a phone, tap the list and tick each topic you want.
   - On a computer, hold **Ctrl** (on a Mac, **Cmd**) and click to pick more than one.
   - Pick the most precise topic: **water / treatment** rather than just **water**.
5. **Text:** write the guide (see "Writing the text" below). You can leave it empty if you attach files.
6. **Language:** a short code: `en` for English, `fr` for French, `es` for Spanish, `pt` for Portuguese, `sw` for Swahili, and so on. It starts as `en`.
7. **License:** leave it as `CC-BY-SA-4.0` unless you have a reason to change it (see "Licenses, simply" below).
8. **Keep it on this NEVO only:** tick it if this guide should stay on this node (see below).
9. **Files** (optional): choose photos, PDFs, videos or other files. You can choose several at once.
10. Press **Add**. Wait until the guide's page appears. Big files take time; keep the page open and the phone near the node until it's done.

If something is missing (a title, a topic, or both text and files), NEVO shows a red message above the form. Your typed text is kept, but **choose your files again**, because browsers forget them.

Limits: at most 100 files and 4 GB in total per guide.

### Writing the text

The text box uses **Markdown**: plain text with a few marks for formatting. The hint above the box shows the most useful ones.

| You type | You get |
|---|---|
| `**boil it**` | **boil it** (bold) |
| `*gently*` | *gently* (slanted) |
| `1. Fill the bottle` on its own line | a numbered step |
| `- a clean cloth` on its own line | a bullet point |
| `## What you need` on its own line | a heading |
| `![the filter](filter.jpg)` | your attached photo `filter.jpg`, shown at that place in the text |
| `[the WHO guide](guide.pdf)` | a link to your attached file `guide.pdf` |

Leave an empty line between paragraphs, and before and after a list.

**Putting photos inside the text.** Attach the photo under **Files**, then write `![a short description](name-of-the-photo.jpg)` where you want it. The description helps people who can't see the photo.

- The name must match the file's name exactly, including capitals and the ending (`.jpg`, `.png`).
- Names without spaces are easiest, such as `filter.jpg`. If a name has spaces, put it between `<` and `>`: `![the filter](<my filter.jpg>)`.
- Give each file a different name. If two have the same name, NEVO renames the second one `name (2).jpg`.

A good guide follows the same order as NEVO's answers:

1. **Warnings first.** What can go wrong, and what not to do.
2. **"You need:"** a list of tools and materials.
3. **Numbered steps.** One action per step. Short sentences.
4. **What to check** at the end, and when to get help.

Example:

```
**Warning:** this does not remove chemicals or salt.

You need:

- A clear plastic bottle (PET), 2 litres or less
- Six hours of sunshine

1. Fill the bottle with the clearest water you have.
2. Close it and lay it flat in full sun.
3. Leave it for 6 hours, or 2 days if the sky is cloudy.

![Bottles on a metal roof](bottles.jpg)
```

### Photos, video and other files

- **Photos:** JPEG, PNG, WebP and GIF are shown in the page. Other kinds (such as iPhone HEIC) are kept, but people must download them to see them.
- **Video:** MP4 and WebM play in the page, and you can skip forward. Other kinds (such as the MOV files many iPhones make) are kept, but people must download them to watch.
- **Audio:** MP3, OGG and WAV play in the page. Other kinds are kept as downloads.
- **PDFs** open in the browser.
- **Anything else** is kept and offered as a download.

Keep videos short and small when you can. Everyone shares the same small computer and its storage.

**Coming soon:** importing large collections from a USB stick, and automatic transcripts of videos so they can be searched.

### "Keep it on this NEVO only"

NEVO nodes will soon share guides with each other, from village to village (**coming soon**). Tick **Keep it on this NEVO only** when a guide matters only here, or shouldn't travel: a local map, the meeting place for this village, notes about one family's well.

Today, nothing is shared yet, so the box only records your choice for later.

Warning: "only on this NEVO" is **not private**. Everyone who can open this NEVO can read it.

### Licenses, simply

A license says what other people may do with your guide.

| License | What it means |
|---|---|
| **CC-BY-SA-4.0** (the default) | Anyone may copy, share and improve your guide, if they name you as the source and share their improved version under the same license. Good knowledge keeps spreading and stays free. |
| **CC-BY-4.0** | Anyone may copy, share and improve it if they name you. Their improved version doesn't have to stay free. |
| **CC0-1.0** | You give it away completely. Anyone may do anything with it, without naming you. |

If you add something you didn't make (a leaflet, a book chapter), use the license its author chose, and only add it if that license allows sharing. Type the license's short name in the License box, such as `CC-BY-4.0`.

## Tips for good guides

- Write for someone who is tired, scared, and reading on a small screen.
- One guide, one job. "Treat a minor burn" is better than "Everything about burns".
- Use photos of each important step.
- Give amounts and times as numbers: "boil for 1 minute", not "boil for a while".
- Say where the knowledge comes from, if it isn't your own experience.
