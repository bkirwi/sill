# Unnamed editor

XXX is text editor for the reMarkable tablet with handwriting input.

## Why a text editor?

The built-in reMarkable notebooks are great for most users who want to write freeform notes, or sometimes export a notebook as text.

However, there are a few special situations where a text editor is essential:

- You want to have precise control over the characters in your document. (For example: you're writing a blog post in Markdown, or programming a short script.)
- You have an existing text file you want to edit. (Copied from your computer, or a system file on the tablet.)

## Why handwriting recognition?

It's already possible to use a normal text editor on reMarkable, via a terminal emulator like Yaft or FingerTerm. These apps use a keyboard for input, either via an onscreen keyboard or one you've hooked up to the tablet somehow.

XXX is different; you enter characters by handwriting directly into the
document. This has a few advantages:

- Handwriting is fun! If you own a reMarkable, you probably already enjoy writing things out by hand.
- It can be faster than using an onscreen keyboard.
- It allows you to enter special characters easily: for example, you can teach XXX to recognize math symbols or accented characters and write them directly in the document.

## How does the handwriting recognition work?

XXX uses "template-based gesture recognition" to recognize the individual characters (letters, digits, punctuation, etc.) that you write. For every character, XXX has a list of templates: examples of what that character looks like when handwritten. When you write a letter on the tablet, XXX looks for the most similar template... and the matching character is inserted into the document.

The contents of the file are displayed on a [French-ruled grid](https://en.wikipedia.org/wiki/Ruled_paper#France), with one character per cell. This makes it easier to write the characters consistently, which in turn makes them easier for the software to recognize. (Nevertheless, it's likely that you'll need to train the handwriting recognizer a little bit before it works reliably for you.)

## Training the handwriting recognition

Some advice for getting the handwriting recognition to work well for you:

- Print, with one character per cell in the grid.
- Try to be consistent with how you write a letter. If you write the letter `Y` in several different ways, XXX will eventually learn all of them... but if you keep it consistent it will work better faster.
- Make sure different characters look different. For example, `l`, `I`, `|` and `1` are often handwritten as a vertical line. You can also use vertical position to differentiate templates; `'` and `,` look similar, but since they're drawn at different places on the grid they're easy for the system to tell apart.


