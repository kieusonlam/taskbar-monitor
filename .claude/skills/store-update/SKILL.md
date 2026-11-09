---
name: store-update
description: Build the MSIX for the current version and submit it as a Microsoft Store update through Partner Center. Usage /store-update
disable-model-invocation: true
---

Ship the version in `Cargo.toml` to the Microsoft Store (product `9MVC9WD60H1S`).

1. Check that this version is already released on GitHub (`gh release view vX.Y.Z`) and that the
   working tree is clean; the Store build must come from the same commit as the tag.
2. `cargo build --release`, then `python store/build.py`. Confirm
   `target/store/TaskbarMonitor_X.Y.Z.0_x64.msix` exists and is well under 10 MB.
3. Ask the user for "What's new in this version" text, or draft it from the commits since the
   previous release and get it approved.
4. In the user's browser, open
   https://partner.microsoft.com/dashboard/products/9MVC9WD60H1S/overview and choose
   **Start update** (or open the draft already in progress). Then:
   - Packages: upload the new .msix, remove the previous package, keep Windows 10/11 Desktop
     checked, save. A "runFullTrust requires approval" warning is expected.
   - Store listings → English (United States): fill "What's new in this version"; update the
     description, features or screenshots if the release changed them. Save.
   - Age ratings: leave unless the release adds online content, user interaction, purchases or
     location sharing; then the IARC questionnaire must be retaken (ask the user before
     agreeing to its terms).
5. Show the user the overview with every section Complete and get an explicit yes before
   pressing **Submit for certification**. Certification takes a few hours to 3 business days.
