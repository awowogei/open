A simpler alternative to xdg-open + xdg-mime

```
Usage: open [OPTIONS] <INPUTS>...

Arguments:
  <INPUTS>...  Paths, urls, mimetypes or filetypes

Options:
  -w, --with <APPLICATION>  Set default application
  -a, --application         Get default application
  -m, --mimetype            Get mimetype
  -h, --help                Print help

Examples:
  Open files:
    open image.png                -- Open a single file
    open *.pdf                    -- Open all pdf files

  Set default application:
    open --with firefox pdf       -- Set default app for a filetype
    open --with firefox file.html -- Set default app for a filetype
    open --with firefox https://  -- Set default app for a protocol
    open --with nvim text/plain   -- Set default app for a mimetype
    open --with nvim "text/*"     -- Set default app for a mime wildcard

  Get default application:
    open --application png        -- Get the default app of a filetype
    open --application file.png   -- Get the default app of a file
    open --application text/html  -- Get the default app of a mimetype

  Get mimetype:
    open --mimetype pdf           -- Get the mimetype of a filetype
    open --mimetype document.pdf  -- Get the mimetype of a file
    open --mimetype https://      -- Get the mimetype of a protocol
```
