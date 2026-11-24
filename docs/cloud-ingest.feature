@spec
Feature: raea cloud: a synced folder is pictured, understood, embedded and organized for the person
  Shane, 2026-11-24: "when i have a data sync, we should auto index/tag and relate them to the appropriate folders or
  have an ai agent or our ask/decision model api if available otherwise use their subscription agent and tag or
  auto-organize the assets or files or folders so we can help understand what purpose they are for and predict what
  they mean/do. also make sure we embed all the assets based on embedding type. this will mean if we have a client
  library, we want to relate a client based folder to that library, or if disorganized we go file by file."
  Later that day: "we should be able to pick a folder from a sync location and have all content when syncing
  auto-tagged and organized for the user", "make sure thumbnails are also generated correctly", and of the models:
  "isn't it running the models in modal?"

  Today (11-24) a sync lists its folder through Composio and adds each file as a link item, all in one flat folder,
  named and typed from its name and mime type only. Search embeds words (Workers AI's bge-small) and nothing else; no
  file is tagged; pictures came from drive.google.com in the viewer's browser, so a viewer signed out of Google saw
  none. The first real client demo is StreetTalk's: workspace streettalk, its Main library's "raea test — StreetTalk
  samples (Nov 2026)" Drive folder (54 files: 47 videos, 6 documents, 1 picture) and three client libraries (Dutch,
  Elfster, The Body Shop), each with a Drive folder of its own.

  Background:
    Given a workspace with Google Drive (or Box, Dropbox, Frame.io, Slack) connected through Composio
    And raea never moves, renames or deletes anything in the tool: organizing happens in raea's library only

  Rule: a folder is picked, and its shape comes with it
    Scenario: IG1. A sync is a folder picked from the tool's own list
      When the person adds a sync and picks a folder from Drive's list (syncadd.js)
      Then the sync keeps the folder's id and its name; a folder typed as its id or its Drive link works the same,
        and is named for the folder on its first run (built 11-24: sync-run.ts driveId, NAME)
    Scenario: IG2. Subfolders come in as folders
      Given the picked folder holds subfolders ("Raw/Angle 1", "Finals")
      Then each becomes a library folder under the sync's folder, with the same name and nesting, and each file lands
        in its own; a file moved between subfolders in the tool moves in raea on the next run

  Rule: every file has a picture anyone who can see the library sees
    Scenario: IG3. A synced file's picture is kept by raea
      When a run lists a file that has a picture in its tool (Drive's thumbnailLink; Box's and Dropbox's thumbnails)
      Then raea fetches it once at 640 px and keeps it in the workspace's own store (items.thumb, the API's signed
        thumb_src), 60 a run, a failed fetch tried again on the next run (built 11-24 for Drive)
      And the library's card shows it, with a play mark on a video; a tool's page address is never put in a <video>
    Scenario: IG4. A file with no picture in its tool gets one made
      Given a video, PDF or document the tool has no picture of
      Then a frame (a video's at 10% of its length), a first page or a typeset title card is made on Modal (IG8) and
        kept the same way

  Rule: every file is understood
    Scenario: IG5. Each new file is read by its kind
      When a run brings files in
      Then each is read once per version (its tool's modified time or checksum): a picture by its picture; a video by
        its picture, frames at 10/50/90% and its words (transcribed); a document by its text (Drive exports Docs as
        text, PDFs are read); a sound by its words or, with none, what it sounds like
      And what it says about the file is kept on the item: a one-line description, tags (its kind: raw footage, b-roll,
        a final cut, a brief, a script, a logo, a voiceover, a reference; its subject; its client or brand; its format,
        9x16 or 16x9; its length), and for each tag who said it and why
    Scenario: IG6. raea's model reads first; the workspace's agent when there's none
      Given the workspace's plan has raea's ask model (raea ask, docs/ask.feature: clef-omni on the plan's pool)
      Then the files are read by it, batched, the spend on the pool
      Otherwise they are read by the workspace's own connected agent on raea cloud's box (Claude Code on its Claude, or
        Antigravity on its Google AI plan), as a background job that never takes a project's turn
      And with neither, files are tagged from their names, folders and types alone, and the library says how to get more
    Scenario: IG7. A person's correction wins
      When someone removes or changes a tag, or moves a file to another folder
      Then raea never puts it back, and the next files like it are read with that correction in the prompt

  Rule: every file is embedded by its kind, once
    Scenario: IG8. Embeddings run on Modal, in raea's own models
      Then pictures and video frames are embedded by SigLIP 2 (the model raea index uses on the Mac, so a desktop's and
        the cloud's vectors compare), sounds by CLAP, and words (documents, transcripts, descriptions) by the text model
        search already uses; on Modal (sdk/cloud/deploy/modal, beside the render farm), a GPU only while there's work
      And a vector is kept per file version and model, never made twice, and dropped with its file
    Scenario: IG9. Search finds a file by what's in it
      When the person searches "girl laughing on a street corner, vertical"
      Then the words are matched against pictures and frames (SigLIP's text tower), descriptions and transcripts, and
        tags, fused into one ranking; a hit says where in a video it is

  Rule: the files are organized for the person
    Scenario: IG10. A folder about a client goes with that client's library
      Given a synced folder whose name, or most of whose files, are about one of the workspace's clients
      Then the sync is related to that client's library (or its files are, when the folder is the workspace's), shown
        with the reason ("47 of 54 files show The Body Shop's products"), and undone in one click
    Scenario: IG11. A disorganized folder is sorted file by file
      Given a folder of files with no useful subfolders
      Then raea proposes folders (by client, then by kind: Raw, B-roll, Finals, Briefs) with each file's place and
        reason, as one review the person accepts whole, or file by file; nothing moves until they do
      And once accepted, new files that fit are placed the same way on later runs

  Rule: the person can see it working
    Scenario: IG12. The left panel says what a sync is doing
      Then a sync bringing files in turns in the left panel, on its row and its library's (built 11-24), then says
        "Reading 12 of 54" while files are read, and its card shows the tags as they come
    Scenario: IG13. Proven on StreetTalk's demo
      When StreetTalk's four Drive syncs run from empty
      Then every file has a picture, a description and tags; the three client folders are related to their client
        libraries; a search for a shot finds it by what's in it; and the run says its time and what it cost
