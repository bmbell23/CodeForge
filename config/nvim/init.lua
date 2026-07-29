-- CodeForge Neovim config (#4).
--
-- This is the editor half of CodeForge's IDE features. CodeForge launches
-- Neovim with NVIM_APPNAME=codeforge, so this config lives at
-- ~/.config/codeforge/init.lua and never touches your personal ~/.config/nvim.
--
-- Provides: fuzzy file finding, project-wide grep ("find all"), a file
-- explorer, syntax via Treesitter, and LSP (go-to-definition, find callers /
-- references, rename). External tools recommended: ripgrep (rg) for live grep,
-- fd for faster file finding, and LSP servers (install via :Mason).

vim.g.mapleader = " "
vim.g.maplocalleader = " "

-- Set 'background' explicitly so Neovim skips its startup terminal-background
-- query (OSC/DSR). In a CodeForge pane that query can go unanswered, printing
-- "E1568: Terminal did not respond to DSR request for 'background' color" and
-- slowing startup; the theme is dark anyway (#64). Override in a local config.
vim.opt.background = "dark"

-- Filetype overrides. Neovim defaults `.pt` to html (Template Toolkit), but in
-- the Janus codebase `.pt` files are Perl — map them so perlnavigator attaches
-- and go-to-def / find-callers work (#37). Override in a local config if your
-- `.pt` files really are templates.
vim.filetype.add({ extension = { pt = "perl" } })

-- Editor keybindings, injected by CodeForge from config.toml's [editor_keys]
-- via the CODEFORGE_EDITOR_KEYS env var as `name=token` lines (#28). One place
-- feeds both the finder/explorer/tab maps and the splash cheatsheet, so a rebind
-- can't leave the splash showing a stale key. Falls back to the built-in tokens
-- when the env is absent (e.g. nvim launched outside CodeForge).
local CFKeys = {}
do
  local defaults = {
    open_file = "Ctrl-p",
    search_in_file = "Ctrl-f",
    search_repo = "Ctrl-Shift-f",
    explorer = "Space e",
    close_tab = "Space b d",
    file_history = "Space g h",
    goto_line = "Space l",
  }
  local raw = vim.env.CODEFORGE_EDITOR_KEYS
  if raw then
    for line in raw:gmatch("[^\n]+") do
      local k, v = line:match("^(%S+)=(.+)$")
      if k and v and v ~= "" then
        defaults[k] = vim.trim(v)
      end
    end
  end

  -- One token atom -> nvim lhs piece. Notation: a modifier joins its key with
  -- "-" (Ctrl-p / Ctrl-Shift-f, short C-/S- also accepted); "Space" is the
  -- leader; anything else is a literal key.
  local function atom_lhs(a)
    if a == "Space" or a == "space" then
      return "<leader>"
    end
    local short = a:gsub("Ctrl%-", "C-"):gsub("Shift%-", "S-"):gsub("Alt%-", "A-"):gsub("Meta%-", "M-")
    if short:match("^[CSAM]%-") then
      return "<" .. short .. ">"
    end
    return a
  end

  -- Token -> nvim keymap lhs. Space-separated atoms concatenate: "Space e" ->
  -- "<leader>e", "Space b d" -> "<leader>bd", "Ctrl-p" -> "<C-p>".
  local function to_lhs(tok)
    local parts = {}
    for a in vim.gsplit(vim.trim(tok), "%s+") do
      if a ~= "" then
        parts[#parts + 1] = atom_lhs(a)
      end
    end
    return table.concat(parts, "")
  end

  -- Token -> human display: expand short modifier forms, leave the rest (long
  -- forms and spaced sequences) as-is. "C-p" -> "Ctrl-p", "Space b d" unchanged.
  local function to_disp(tok)
    tok = vim.trim(tok)
    tok = tok:gsub("C%-", "Ctrl-"):gsub("S%-", "Shift-"):gsub("A%-", "Alt-"):gsub("M%-", "Meta-")
    return tok
  end

  CFKeys.tok = defaults
  CFKeys.lhs = function(name)
    return to_lhs(defaults[name])
  end
  CFKeys.disp = function(name)
    return to_disp(defaults[name])
  end

  -- CodeForge prefix bindings (the "Ctrl-a X" keys), injected the same way, so
  -- the splash cheatsheet's prefix keys stay live after a Ctrl-a ? rebind.
  local prefix = to_disp(vim.env.CODEFORGE_PREFIX or "C-a")
  local pkeys = {
    toggle_editor = "e", toggle_shell = "t", toggle_ai = "c",
    git_diff = "g", win_new = "n", copy = "v", help = "?",
    tab_next = "]", tab_prev = "[",
    focus_left = "h", focus_down = "j", focus_up = "k", focus_right = "l",
  }
  local praw = vim.env.CODEFORGE_PREFIX_KEYS
  if praw then
    for line in praw:gmatch("[^\n]+") do
      local k, v = line:match("^(%S+)=(.+)$")
      if k and v and v ~= "" then
        pkeys[k] = v
      end
    end
  end
  CFKeys.prefix = prefix
  -- Display a prefix binding, e.g. "Ctrl-a g". Pass several names to join their
  -- keys after one prefix, e.g. pdisp("toggle_editor","toggle_shell") -> "Ctrl-a e/t".
  CFKeys.pdisp = function(...)
    local parts = {}
    for _, name in ipairs({ ... }) do
      parts[#parts + 1] = pkeys[name] or "?"
    end
    return prefix .. " " .. table.concat(parts, "/")
  end
end

-- Sensible baseline.
local opt = vim.opt
opt.number = true
opt.relativenumber = false -- show real (absolute) line numbers on every line
opt.mouse = "a"
-- Right-click opens a context menu (PopUp) at the click, moving the cursor there
-- first so LSP actions target the symbol under the pointer (#31).
opt.mousemodel = "popup_setpos"
-- Open that same PopUp menu at the cursor with no mouse (#34). `:popup!` (with
-- the bang) anchors at the text cursor instead of the mouse pointer; the menu's
-- LSP entries are the global ones added on LspAttach, so this shares #31's menu.
vim.keymap.set("n", "<leader>m", "<Cmd>popup! PopUp<CR>", { desc = "Context menu at cursor" })

-- Notes: Ctrl-s (and <leader>S) in Notes-repo buffers writes all and kicks the
-- sync script (commit + pull + push) immediately — the "Save now" button (#71).
-- Works because CodeForge runs the terminal raw (flow control off), so Ctrl-s
-- reaches nvim instead of freezing. Bound only under the Notes dir, which
-- CodeForge passes in via env.
do
  local notes_dir = vim.env.CODEFORGE_NOTES_DIR
  local notes_sync = vim.env.CODEFORGE_NOTES_SYNC
  if notes_dir and notes_dir ~= "" and notes_sync and notes_sync ~= "" then
    -- Ctrl-s: write all + sync now.
    local function cf_notes_save()
      vim.cmd("silent! wall")
      vim.fn.jobstart({ notes_sync, "--now" }, { detach = true })
      vim.notify("Notes: saving…")
    end
    local function cf_sync()
      vim.fn.jobstart({ notes_sync, "--now" }, { detach = true })
    end
    -- Alt-s: rename + move the current note within the repo (#72). One path
    -- field, prefilled with the current path relative to the Notes root, with
    -- file/dir tab-completion; missing dirs are created; the sync pushes it.
    local function cf_notes_save_as()
      local cur = vim.api.nvim_buf_get_name(0)
      if cur == "" then
        return
      end
      local rel = cur:gsub("^" .. vim.pesc(notes_dir) .. "/", "")
      vim.ui.input(
        { prompt = "Save note as (path in Notes): ", default = rel, completion = "file" },
        function(input)
          if not input or vim.trim(input) == "" then
            return
          end
          input = vim.trim(input)
          if not input:match("%.md$") then
            input = input .. ".md"
          end
          local dest = notes_dir .. "/" .. input
          if dest == cur then
            return
          end
          if vim.fn.filereadable(dest) == 1 then
            vim.notify("Target already exists: " .. input, vim.log.levels.WARN)
            return
          end
          vim.fn.mkdir(vim.fn.fnamemodify(dest, ":h"), "p")
          vim.cmd("silent! write") -- persist current content to the old path first
          local oldbuf = vim.api.nvim_get_current_buf()
          -- git mv keeps history when tracked; fall back to a filesystem move.
          vim.fn.system({ "git", "-C", notes_dir, "mv", "--", cur, dest })
          if vim.v.shell_error ~= 0 then
            if vim.fn.filereadable(cur) == 1 then
              (vim.uv or vim.loop).fs_rename(cur, dest)
            else
              vim.cmd("write " .. vim.fn.fnameescape(dest))
            end
          end
          vim.cmd("keepalt edit " .. vim.fn.fnameescape(dest))
          pcall(vim.api.nvim_buf_delete, oldbuf, { force = true })
          cf_sync()
          vim.notify("Note → " .. input)
        end
      )
    end
    -- Alt-d: delete the current note (a throwaway scratchpad). Removes the file
    -- (git rm when tracked) and pushes the deletion; the window is never left
    -- empty (opens a fresh note if this was the last one) (#72).
    local function cf_notes_delete()
      local cur = vim.api.nvim_buf_get_name(0)
      if vim.fn.confirm("Delete this note?", "&Yes\n&No", 2) ~= 1 then
        return
      end
      local oldbuf = vim.api.nvim_get_current_buf()
      local listed = 0
      for _, b in ipairs(vim.api.nvim_list_bufs()) do
        if vim.bo[b].buflisted then
          listed = listed + 1
        end
      end
      if listed > 1 then
        vim.cmd("silent! bprevious")
      else
        local t = os.date("*t")
        local day = string.format("%s/History/%04d/%02d/%02d", notes_dir, t.year, t.month, t.day)
        vim.fn.mkdir(day, "p")
        local fresh = string.format(
          "%s/%04d-%02d-%02d-%02d:%02d:%02d.md",
          day,
          t.year,
          t.month,
          t.day,
          t.hour,
          t.min,
          t.sec
        )
        vim.cmd("edit " .. vim.fn.fnameescape(fresh))
      end
      pcall(vim.api.nvim_buf_delete, oldbuf, { force = true })
      if cur ~= "" then
        vim.fn.system({ "git", "-C", notes_dir, "rm", "-f", "--quiet", "--", cur })
        if vim.v.shell_error ~= 0 and vim.fn.filereadable(cur) == 1 then
          (vim.uv or vim.loop).fs_unlink(cur)
        end
        cf_sync() -- commit the deletion + push, if it was committed
      end
      vim.notify("Note deleted")
    end
    vim.api.nvim_create_autocmd({ "BufReadPost", "BufNewFile" }, {
      pattern = notes_dir .. "/*",
      callback = function(ev)
        local o = { buffer = ev.buf }
        vim.keymap.set({ "n", "i" }, "<C-s>", cf_notes_save, vim.tbl_extend("force", o, { desc = "Save + sync notes" }))
        vim.keymap.set("n", "<leader>S", cf_notes_save, vim.tbl_extend("force", o, { desc = "Save + sync notes" }))
        vim.keymap.set({ "n", "i" }, "<M-s>", cf_notes_save_as, vim.tbl_extend("force", o, { desc = "Notes: save as / rename+move" }))
        vim.keymap.set("n", "<M-d>", cf_notes_delete, vim.tbl_extend("force", o, { desc = "Notes: delete this note" }))
      end,
    })
  end
end

-- Close the current Notes note (mod-x); if it's the last buffer, open the fresh
-- timestamped note CodeForge passes in first, so the Notes window is never left
-- empty (#70). Called over RPC from forge's tab_close handler.
function _G.CF_notes_close(newpath)
  local listed = 0
  for _, b in ipairs(vim.api.nvim_list_bufs()) do
    if vim.bo[b].buflisted then
      listed = listed + 1
    end
  end
  if listed > 1 then
    vim.cmd("silent! bprevious | bdelete #")
  else
    vim.cmd("edit " .. vim.fn.fnameescape(newpath))
    vim.cmd("silent! bdelete #")
  end
end

-- Jump to a line by number (#59): press the goto_line key, then type digits —
-- the cursor moves live after each one (8 -> 81 -> 810), no Enter needed. Any
-- non-digit (Esc, Enter, ...) ends it, leaving the cursor where it landed.
-- Out-of-range numbers clamp to the last line.
local function cf_goto_line()
  local n = ""
  while true do
    local ok, ch = pcall(vim.fn.getcharstr)
    if not ok or not ch:match("^%d$") then
      break
    end
    n = n .. ch
    local total = vim.api.nvim_buf_line_count(0)
    local line = math.min(math.max(tonumber(n), 1), total)
    pcall(vim.api.nvim_win_set_cursor, 0, { line, 0 })
    pcall(vim.cmd, "normal! ^") -- land on first non-blank, like `G`
    vim.cmd("redraw")
  end
end
vim.keymap.set("n", CFKeys.lhs("goto_line"), cf_goto_line, { desc = "Jump to line (type digits, live)" })
opt.clipboard = "unnamedplus"
opt.ignorecase = true
opt.smartcase = true
opt.termguicolors = true
opt.signcolumn = "yes"
opt.updatetime = 250
opt.splitright = true
opt.splitbelow = true
opt.expandtab = true
opt.shiftwidth = 4
opt.tabstop = 4
opt.scrolloff = 6

-- Bootstrap lazy.nvim. Verify the actual entry file, not just the directory: a
-- failed/partial clone can leave the dir behind, which later surfaces as the
-- cryptic "module 'lazy' not found" and unloads every plugin (#63).
local uv = vim.uv or vim.loop
local lazypath = vim.fn.stdpath("data") .. "/lazy/lazy.nvim"
local lazymain = lazypath .. "/lua/lazy/init.lua"
if not uv.fs_stat(lazymain) then
  local repo = "https://github.com/folke/lazy.nvim.git"
  -- Blobless clone is fast; fall back to a full clone for gits too old for
  -- --filter, so a partial-clone failure doesn't leave the editor plugin-less.
  local out = vim.fn.system({ "git", "clone", "--filter=blob:none", "--branch=stable", repo, lazypath })
  if vim.v.shell_error ~= 0 then
    out = vim.fn.system({ "git", "clone", "--branch=stable", repo, lazypath })
  end
  if not uv.fs_stat(lazymain) then
    vim.api.nvim_echo({
      { "CodeForge: could not bootstrap lazy.nvim into " .. lazypath .. "\n", "ErrorMsg" },
      { (out or "") .. "\n", "WarningMsg" },
      { "Check that `git` is installed and github.com is reachable, then relaunch.\n", "WarningMsg" },
    }, true, {})
    return
  end
end
vim.opt.rtp:prepend(lazypath)

require("lazy").setup({
  -- Colorscheme.
  {
    "folke/tokyonight.nvim",
    priority = 1000,
    config = function()
      pcall(vim.cmd.colorscheme, "tokyonight-night")
    end,
  },

  -- Fuzzy finder: files, live grep, buffers.
  {
    "nvim-telescope/telescope.nvim",
    branch = "0.1.x",
    dependencies = { "nvim-lua/plenary.nvim" },
    config = function()
      -- Telescope 0.1.x calls nvim-treesitter's old `ft_to_lang`, which the
      -- rewritten (main-branch) nvim-treesitter removed. That killed live_grep /
      -- find_files with a nil-call in __files.lua:438 and previewers/utils.lua.
      -- Disabling preview treesitter isn't enough (the __files path isn't gated
      -- by it), so restore the function as a thin shim over the new API.
      pcall(function()
        local p = require("nvim-treesitter.parsers")
        if type(p.ft_to_lang) ~= "function" then
          p.ft_to_lang = function(ft)
            return vim.treesitter.language.get_lang(ft) or ft
          end
        end
      end)
      -- The same rewrite DELETED the whole `nvim-treesitter.configs` module,
      -- which the 0.1.x file previewer (__files.lua:445) still requires and
      -- calls `is_enabled` on. `require` errors -> telescope's pcall leaves a
      -- string, and `("err"):is_enabled()` is a nil-call -> Ctrl-F/live_grep
      -- crash. A field-shim can't help (require never returns a table), so
      -- register a stub module in package.loaded. Preview treesitter is off
      -- anyway, so is_enabled=false is correct.
      if type(package.loaded["nvim-treesitter.configs"]) ~= "table" then
        package.loaded["nvim-treesitter.configs"] = {
          is_enabled = function()
            return false
          end,
        }
      end
      require("telescope").setup({
        defaults = {
          preview = { treesitter = false },
          -- Never list the .git internals even when hidden files are shown.
          file_ignore_patterns = { "%.git/" },
        },
        -- Show dotfiles (e.g. .gitignore) in the fuzzy file finder; gitignored
        -- paths (target/, node_modules/) stay hidden via the default respect.
        pickers = { find_files = { hidden = true } },
      })
      local t = require("telescope.builtin")

      -- Find-in-file (Ctrl-f) remembers its last query so reopening prefills it
      -- (#53); Ctrl-u in the prompt clears. Find-all (Ctrl-g) is grug-far — a
      -- live, grouped-by-file grep with a replace field (its own spec below).
      local last_buf = ""
      local tstate = require("telescope.actions.state")
      -- Switch the in-file search into a find & replace scoped to this file
      -- (#52): hand the current query to grug-far with its Paths field pinned to
      -- the file, so Ctrl-f leads into replace the same way Ctrl-g does repo-wide.
      -- One reused instance, refreshed to the current file/query each time.
      local function cf_replace_in_file(file, query)
        if file == "" then
          return
        end
        local gf = require("grug-far")
        local name = "codeforge-replace"
        local prefills = { search = query, paths = file }
        if gf.has_instance(name) then
          local inst = gf.get_instance(name)
          inst:open()
          inst:update_input_values(prefills, true)
        else
          gf.open({ instanceName = name, prefills = prefills })
        end
      end
      local function cf_find_in_file()
        local file = vim.api.nvim_buf_get_name(0)
        t.current_buffer_fuzzy_find({
          default_text = last_buf,
          attach_mappings = function(_, map)
            local actions = require("telescope.actions")
            map({ "i", "n" }, "<CR>", function(pb)
              last_buf = tstate.get_current_line() or last_buf
              actions.select_default(pb)
            end)
            -- Ctrl-r: replace within this file, prefilled with the query so far.
            map({ "i", "n" }, "<C-r>", function(pb)
              local query = tstate.get_current_line() or ""
              last_buf = query
              actions.close(pb)
              cf_replace_in_file(file, query)
            end)
            return true
          end,
        })
      end

      -- VS Code-style (work in normal & insert):
      --   Ctrl-P  open file by name
      --   Ctrl-F  search within the current file (remembers last, #53)
      --   Ctrl-G  find all across the repo -> grug-far (live, grouped by file,
      --           with replace) — bound below where grug-far is available.
      vim.keymap.set({ "n", "i" }, CFKeys.lhs("open_file"), t.find_files, { desc = "Open file" })
      vim.keymap.set({ "n", "i" }, CFKeys.lhs("search_in_file"), cf_find_in_file, { desc = "Search in file" })
      vim.keymap.set("n", "<leader>ff", t.find_files, { desc = "Find files" })
      vim.keymap.set("n", "<leader>fb", t.buffers, { desc = "Find buffers" })
      vim.keymap.set("n", "<leader>fh", t.help_tags, { desc = "Find help" })
      vim.keymap.set("n", "<leader>fs", t.current_buffer_fuzzy_find, { desc = "Search in file" })
      vim.keymap.set("n", "<leader>fw", t.grep_string, { desc = "Find word under cursor" })
      vim.keymap.set("n", "<leader>fr", t.resume, { desc = "Resume last search" })
    end,
  },

  -- File explorer as an editable buffer.
  {
    "stevearc/oil.nvim",
    opts = {
      default_file_explorer = true,
      view_options = { show_hidden = true },
      -- oil is a buffer-based explorer (you navigate in/out of dirs, not an
      -- expand-in-place tree). Map the intuitive keys on top of the defaults:
      --   Backspace / Left  -> go up to the parent directory
      --   Right             -> enter the dir / open the file under the cursor
      keymaps = {
        ["<BS>"] = "actions.parent",
        ["<Left>"] = "actions.parent",
        ["<Right>"] = "actions.select",
      },
    },
    config = function(_, opts)
      require("oil").setup(opts)
      vim.keymap.set("n", CFKeys.lhs("explorer"), "<cmd>Oil<cr>", { desc = "File explorer" })
      vim.keymap.set("n", "-", "<cmd>Oil<cr>", { desc = "Open parent dir" })
    end,
  },

  -- Visible buffer tabs across the top, so several open files (e.g. one opened
  -- by go-to-definition) are tabbable like an editor (Story #11 / #12 editor
  -- side). CodeForge keeps the editor as a single nvim; these are nvim buffers.
  {
    "akinsho/bufferline.nvim",
    config = function()
      vim.opt.termguicolors = true
      require("bufferline").setup({
        options = {
          diagnostics = "nvim_lsp",
          -- No icons: filetype/close glyphs need a Nerd Font and otherwise
          -- render as empty [] boxes in a normal terminal.
          show_buffer_icons = false,
          show_buffer_close_icons = false,
          show_close_icon = false,
          separator_style = "thin",
          -- Don't render a tab for buffers that aren't a real file — the initial
          -- empty [No Name] and the CodeForge splash. Otherwise a janky blank /
          -- "[No Name]" tab shows next to the splash.
          custom_filter = function(bufnr)
            if vim.api.nvim_buf_get_name(bufnr) == "" then
              return false
            end
            if vim.bo[bufnr].filetype == "alpha" then
              return false
            end
            return true
          end,
          -- Right-click a tab -> path/close menu for that buffer (#32).
          right_mouse_command = "lua CF_tab_menu(%d)",
        },
      })
      -- Cycle / reorder / close buffers. ]b [b move; <leader>bp pins a picker.
      -- Next/prev tab primarily use the CodeForge prefix keys (Ctrl-a ] / [),
      -- which cycle these same buffers over RPC. Keep ]b/[b as in-nvim aliases.
      vim.keymap.set("n", "]b", "<cmd>BufferLineCycleNext<cr>", { desc = "Next buffer/tab" })
      vim.keymap.set("n", "[b", "<cmd>BufferLineCyclePrev<cr>", { desc = "Prev buffer/tab" })
      vim.keymap.set("n", "<S-l>", "<cmd>BufferLineCycleNext<cr>", { desc = "Next buffer/tab" })
      vim.keymap.set("n", "<S-h>", "<cmd>BufferLineCyclePrev<cr>", { desc = "Prev buffer/tab" })
      vim.keymap.set("n", "<leader>bp", "<cmd>BufferLinePick<cr>", { desc = "Pick a buffer/tab" })
      -- Close the current buffer/tab. On the LAST real buffer, :bdelete would
      -- leave a blank [No Name] (looks like nothing closed) — drop to the splash
      -- instead so closing the last tab is visible and useful.
      vim.keymap.set("n", CFKeys.lhs("close_tab"), function()
        if vim.bo.modified then
          vim.notify("Unsaved changes — :w (or <leader>w) first", vim.log.levels.WARN)
          return
        end
        local listed = vim.tbl_filter(function(b)
          return vim.bo[b].buflisted
        end, vim.api.nvim_list_bufs())
        vim.cmd("bdelete")
        if #listed <= 1 and _G.CodeForgeSplash then
          _G.CodeForgeSplash()
        end
      end, { desc = "Close buffer/tab" })
      -- Jump straight to tab N.
      for i = 1, 9 do
        vim.keymap.set(
          "n",
          "<leader>" .. i,
          "<cmd>BufferLineGoToBuffer " .. i .. "<cr>",
          { desc = "Go to buffer/tab " .. i }
        )
      end

      -- Tab context menu (#32): right-click a tab, or <leader>bm for the current
      -- one. Copies emit OSC 52 so the path reaches the system clipboard over
      -- SSH (forge forwards pane OSC 52 to the real terminal).
      local function cf_clip(text, label)
        vim.fn.setreg("+", text)
        vim.fn.setreg('"', text)
        local ok, osc = pcall(require, "vim.ui.clipboard.osc52")
        if ok then
          pcall(osc.copy("+"), { text })
        end
        vim.notify((label or "Copied") .. ": " .. text)
      end
      local function cf_close_buf(bufnr)
        if vim.bo[bufnr].modified then
          vim.notify("Unsaved changes — :w first", vim.log.levels.WARN)
          return
        end
        local listed = vim.tbl_filter(function(b)
          return vim.bo[b].buflisted
        end, vim.api.nvim_list_bufs())
        pcall(vim.api.nvim_buf_delete, bufnr, {})
        if #listed <= 1 and _G.CodeForgeSplash then
          _G.CodeForgeSplash()
        end
      end
      function _G.CF_tab_menu(bufnr)
        bufnr = (bufnr and bufnr ~= 0) and bufnr or vim.api.nvim_get_current_buf()
        local path = vim.api.nvim_buf_get_name(bufnr)
        if path == "" then
          vim.notify("No file for this tab", vim.log.levels.WARN)
          return
        end
        local name = vim.fn.fnamemodify(path, ":t")
        local items = {
          { "Copy absolute path", function() cf_clip(vim.fn.fnamemodify(path, ":p"), "Copied path") end },
          -- ":." is relative to nvim's cwd, which forge sets to the project root.
          { "Copy relative path", function() cf_clip(vim.fn.fnamemodify(path, ":."), "Copied path") end },
          { "Copy file name", function() cf_clip(name, "Copied name") end },
          { "Close tab", function() cf_close_buf(bufnr) end },
          { "Close others", function()
            for _, b in ipairs(vim.api.nvim_list_bufs()) do
              if b ~= bufnr and vim.bo[b].buflisted then
                pcall(vim.api.nvim_buf_delete, b, {})
              end
            end
          end },
        }
        vim.ui.select(items, {
          prompt = "Tab: " .. name,
          format_item = function(it)
            return it[1]
          end,
        }, function(choice)
          if choice then
            choice[2]()
          end
        end)
      end
      vim.keymap.set("n", "<leader>bm", function()
        CF_tab_menu(0)
      end, { desc = "Tab actions menu" })
    end,
  },

  -- CodeForge start screen: a project-aware splash when the editor opens with
  -- no file. Shows the project + git context, recent files you can jump to by
  -- number, and a key cheatsheet — and you can just start typing to fuzzy-open
  -- a file (the first key seeds Telescope find_files).
  {
    "goolord/alpha-nvim",
    -- Load eagerly, NOT on VimEnter: alpha registers its auto-draw inside its
    -- own VimEnter handler, so lazy-loading it on VimEnter misses the event and
    -- nvim shows its stock intro instead of our splash.
    lazy = false,
    priority = 100,
    config = function()
      local ok, alpha = pcall(require, "alpha")
      if not ok then
        return
      end
      local db = require("alpha.themes.dashboard")
      local cwd = vim.fn.getcwd()

      local header = {
        [[   ______          __     ______                     ]],
        [[  / ____/___  ____/ /__  / ____/___  _________ ____   ]],
        [[ / /   / __ \/ __  / _ \/ /_  / __ \/ ___/ __ `/ _ \  ]],
        [[/ /___/ /_/ / /_/ /  __/ __/ / /_/ / /  / /_/ /  __/  ]],
        [[\____/\____/\__,_/\___/_/    \____/_/   \__, /\___/   ]],
        [[                                       /____/         ]],
        [[         terminal-native IDE · nvim + shell + Claude  ]],
      }

      -- The splash body (git context, changed files, recents) is dynamic and
      -- must be rebuilt on every draw: at plugin-config time the shada hasn't
      -- been read yet, so v:oldfiles is still empty and recents would never
      -- show. Rebuilding also keeps the changed-files list current.
      local function splash_setup()
        -- Project + git context line.
        local function git_bits()
          local br = vim.fn.systemlist({ "git", "-C", cwd, "branch", "--show-current" })
          if vim.v.shell_error ~= 0 or not br[1] or br[1] == "" then
            return nil, {}
          end
          local st = vim.fn.systemlist({ "git", "-C", cwd, "status", "--porcelain" })
          return br[1], (vim.v.shell_error == 0) and st or {}
        end
        local branch, changed = git_bits()
        local ctx = "  " .. vim.fn.fnamemodify(cwd, ":t")
        if branch then
          ctx = ctx .. "   ⎇ " .. branch
          if #changed > 0 then
            ctx = ctx .. "   " .. #changed .. " changed"
          end
        end

        -- Recent files, project-local first, that still exist.
        local recent = {}
        do
          local under, other, seen = {}, {}, {}
          for _, f in ipairs(vim.v.oldfiles or {}) do
            if not seen[f] and vim.fn.filereadable(f) == 1 then
              seen[f] = true
              if f:sub(1, #cwd) == cwd then
                under[#under + 1] = f
              else
                other[#other + 1] = f
              end
            end
          end
          vim.list_extend(recent, under)
          vim.list_extend(recent, other)
        end
        -- Everything below the context line is padded to one shared width (W)
        -- before centering. Alpha centers each line on its own width, so equal
        -- widths are what give the body a single left edge — unequal lines are
        -- what made the old cheatsheet read as scattered text.
        local MAXW = 76

        -- Trim long paths from the left, keeping the informative tail.
        local function clip(s, w)
          if vim.fn.strdisplaywidth(s) <= w then
            return s
          end
          while vim.fn.strdisplaywidth(s) > w - 1 do
            s = vim.fn.strcharpart(s, 1)
          end
          return "…" .. s
        end

        -- Cheatsheet: {category, {key, what}, ...}, laid out two pairs per row
        -- with per-column widths so keys and descriptions form real columns.
        -- Keys come from CFKeys so the splash always matches the live bindings
        -- (editor keys from [editor_keys], prefix keys from the running config).
        local sheet = {
          {
            "Files",
            { CFKeys.disp("open_file"), "open file" },
            { CFKeys.disp("explorer"), "file tree" },
            { CFKeys.disp("search_repo"), "search text" },
          },
          {
            "Editor",
            { "gd", "go to definition" },
            { "gr", "references" },
            { CFKeys.pdisp("tab_next", "tab_prev"), "next/prev file" },
          },
          {
            "Git",
            { CFKeys.pdisp("git_diff"), "git diff" },
            { CFKeys.disp("file_history"), "file history" },
          },
          {
            "Panes",
            { CFKeys.pdisp("toggle_editor", "toggle_shell", "toggle_ai"), "editor/term/claude" },
            { CFKeys.pdisp("focus_left", "focus_down", "focus_up", "focus_right"), "move focus" },
          },
          {
            "Session",
            { CFKeys.pdisp("win_new"), "new window" },
            { CFKeys.pdisp("copy"), "scroll/copy" },
            { CFKeys.pdisp("help"), "all keys" },
          },
        }
        local rows = {}
        for _, s in ipairs(sheet) do
          for i = 2, #s, 2 do
            rows[#rows + 1] = { i == 2 and s[1] or "", s[i], s[i + 1] }
          end
        end
        local catw, kw, dw = 0, { 0, 0 }, { 0, 0 }
        for _, row in ipairs(rows) do
          catw = math.max(catw, #row[1])
          for c = 1, 2 do
            if row[c + 1] then
              kw[c] = math.max(kw[c], #row[c + 1][1])
              dw[c] = math.max(dw[c], #row[c + 1][2])
            end
          end
        end
        local key_lines, key_hls = {}, {}
        for _, row in ipairs(rows) do
          local line = string.format("%-" .. catw .. "s", row[1])
          local hls = {}
          if row[1] ~= "" then
            hls[#hls + 1] = { "Function", 0, #row[1] }
          end
          for c = 1, 2 do
            local p = row[c + 1]
            if p then
              local ks = #line + 3
              line = line
                .. "   "
                .. string.format("%-" .. kw[c] .. "s", p[1])
                .. "  "
                .. string.format("%-" .. dw[c] .. "s", p[2])
              hls[#hls + 1] = { "Special", ks, ks + #p[1] }
              hls[#hls + 1] = { "Comment", ks + kw[c] + 2, ks + kw[c] + 2 + #p[2] }
            end
          end
          key_lines[#key_lines + 1] = line
          key_hls[#key_hls + 1] = hls
        end

        -- Changed files (git status), capped so the splash stays one screen.
        local changed_rows = {}
        for i = 1, math.min(#changed, 4) do
          local l = changed[i]
          changed_rows[#changed_rows + 1] = { l:sub(1, 2), clip(vim.trim(l:sub(4)), MAXW - 4) }
        end
        if #changed > 4 then
          changed_rows[#changed_rows + 1] = { "", ("+ %d more"):format(#changed - 4) }
        end

        local rec_rows = {}
        for i = 1, math.min(#recent, 6) do
          rec_rows[i] = { f = recent[i], disp = clip(vim.fn.fnamemodify(recent[i], ":~:."), MAXW - 3) }
        end

        local W = 0
        for _, l in ipairs(key_lines) do
          W = math.max(W, vim.fn.strdisplaywidth(l))
        end
        for _, r in ipairs(changed_rows) do
          W = math.max(W, 4 + vim.fn.strdisplaywidth(r[2]))
        end
        for _, r in ipairs(rec_rows) do
          W = math.max(W, 3 + vim.fn.strdisplaywidth(r.disp))
        end

        local function padW(s)
          return s .. string.rep(" ", math.max(0, W - vim.fn.strdisplaywidth(s)))
        end
        local function text(lines, hl)
          return { type = "text", val = lines, opts = { position = "center", hl = hl } }
        end
        -- One body line, padded to W, with {group, byte_start, byte_end} spans.
        local function line_el(s, spans)
          return { type = "text", val = padW(s), opts = { position = "center", hl = spans } }
        end

        local changed_els = {}
        for _, r in ipairs(changed_rows) do
          local l = string.format("%-4s%s", r[1], r[2])
          local grp = (r[1] ~= "") and "WarningMsg" or "Comment"
          changed_els[#changed_els + 1] = line_el(l, { { grp, 0, (r[1] ~= "") and #r[1] or #l } })
        end

        local buttons = {}
        for i, r in ipairs(rec_rows) do
          local b = db.button(tostring(i), i .. "  " .. r.disp, "<cmd>edit " .. vim.fn.fnameescape(r.f) .. "<cr>")
          b.opts.width = W
          b.opts.shortcut = "" -- the number is inline in the label instead
          b.opts.hl = { { "Special", 0, #tostring(i) } }
          buttons[i] = b
        end

        local footer = { padW("just start typing to fuzzy-open a file") }

        local layout = {
          { type = "padding", val = 2 },
          text(header, "Keyword"),
          { type = "padding", val = 1 },
          text({ ctx }, "Identifier"),
          { type = "padding", val = 1 },
        }
        if #changed_els > 0 then
          layout[#layout + 1] = line_el("changed now", { { "Comment", 0, 11 } })
          vim.list_extend(layout, changed_els)
          layout[#layout + 1] = { type = "padding", val = 1 }
        end
        if #buttons > 0 then
          local h = "recent — press number to open"
          layout[#layout + 1] = line_el(h, { { "Comment", 0, #h } })
          layout[#layout + 1] = { type = "group", val = buttons, opts = { spacing = 0 } }
          layout[#layout + 1] = { type = "padding", val = 1 }
        end
        for i, l in ipairs(key_lines) do
          layout[#layout + 1] = line_el(l, key_hls[i])
        end
        layout[#layout + 1] = { type = "padding", val = 1 }
        layout[#layout + 1] = text(footer, "Function")
        alpha.setup({ layout = layout, opts = { margin = 5 } })
      end
      splash_setup()

      -- Redraw the splash on demand (e.g. after closing the last buffer). Rebuild
      -- first so git context / recents are current.
      _G.CodeForgeSplash = function()
        splash_setup()
        require("alpha").start(false)
      end

      -- Draw the splash ourselves on an empty start (alpha's built-in autostart
      -- proved unreliable here). Guarded so opening a file never shows it.
      vim.api.nvim_create_autocmd("VimEnter", {
        once = true,
        callback = function()
          if
            vim.fn.argc() == 0
            and vim.api.nvim_buf_get_name(0) == ""
            and vim.bo.buftype == ""
            and vim.api.nvim_buf_line_count(0) <= 1
            and (vim.api.nvim_buf_get_lines(0, 0, 1, false)[1] or "") == ""
          then
            -- Rebuild first: the shada (v:oldfiles → recents) is only read
            -- after plugin config ran. start(false) forces a draw; start(true)
            -- (the on-vimenter path) proved to be a no-op with this alpha
            -- version.
            splash_setup()
            require("alpha").start(false)
          end
        end,
      })

      -- Type-to-open: any letter opens the fuzzy finder seeded with it. Digits
      -- stay bound to the recent-file buttons above.
      vim.api.nvim_create_autocmd("FileType", {
        pattern = "alpha",
        callback = function(ev)
          local tb = require("telescope.builtin")
          local chars = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ._-"
          for i = 1, #chars do
            local ch = chars:sub(i, i)
            vim.keymap.set("n", ch, function()
              tb.find_files({ default_text = ch })
            end, { buffer = ev.buf, nowait = true, silent = true })
          end
          vim.keymap.set("n", "<CR>", tb.find_files, { buffer = ev.buf, nowait = true })
        end,
      })

      -- When the last real file buffer is closed (e.g. Ctrl-a w on the editor),
      -- fall back to this splash instead of a blank [No Name] buffer.
      vim.api.nvim_create_autocmd("BufDelete", {
        callback = function()
          vim.schedule(function()
            local files = 0
            for _, b in ipairs(vim.api.nvim_list_bufs()) do
              if
                vim.api.nvim_buf_is_loaded(b)
                and vim.bo[b].buflisted
                and vim.bo[b].buftype == ""
                and vim.api.nvim_buf_get_name(b) ~= ""
              then
                files = files + 1
              end
            end
            -- Only take over an empty scratch buffer, never a terminal/oil/etc.
            if
              files == 0
              and vim.bo.buftype == ""
              and vim.api.nvim_buf_get_name(0) == ""
              and vim.bo.filetype ~= "alpha"
            then
              splash_setup() -- refresh recents + changed files
              require("alpha").start(false)
            end
          end)
        end,
      })

      -- Forge can kill the editor pane without a clean nvim exit, which loses
      -- the shada update and with it v:oldfiles — the splash's recent list was
      -- starving because of this. Merge-write the shada on every file open so
      -- recents survive hard kills.
      vim.api.nvim_create_autocmd("BufReadPost", {
        callback = function()
          vim.schedule(function()
            pcall(vim.cmd, "wshada")
          end)
        end,
      })

      -- Drop the initial empty [No Name] buffer once a real file is open, so it
      -- doesn't linger as a blank, useless bufferline tab (#33). Only wipes an
      -- unnamed, unmodified, empty, ordinary buffer shown in no window — never
      -- the splash (ft=alpha), a terminal, or a real file.
      vim.api.nvim_create_autocmd("BufReadPost", {
        callback = function()
          if vim.api.nvim_buf_get_name(0) == "" or vim.bo.buftype ~= "" then
            return -- what just opened isn't a real file; nothing to clean up
          end
          vim.schedule(function()
            for _, b in ipairs(vim.api.nvim_list_bufs()) do
              if
                vim.api.nvim_buf_is_loaded(b)
                and vim.bo[b].buflisted
                and vim.bo[b].buftype == ""
                and vim.api.nvim_buf_get_name(b) == ""
                and not vim.bo[b].modified
                and vim.api.nvim_buf_line_count(b) == 1
                and (vim.api.nvim_buf_get_lines(b, 0, 1, false)[1] or "") == ""
                and #vim.fn.win_findbuf(b) == 0
              then
                pcall(vim.api.nvim_buf_delete, b, {})
              end
            end
          end)
        end,
      })
    end,
  },

  -- Git diff (#18): a GitLens-lite view. `Space g d` opens a changed-files
  -- panel; pick one for a side-by-side diff (old left, working tree right,
  -- editable). `q` or `Esc` closes it. `Space g h` shows a file's history.
  {
    "sindrets/diffview.nvim",
    dependencies = { "nvim-lua/plenary.nvim" },
    cmd = { "DiffviewOpen", "DiffviewClose", "DiffviewFileHistory", "DiffviewToggleFiles" },
    config = function()
      local close = "<cmd>DiffviewClose<cr>"
      require("diffview").setup({
        use_icons = false, -- no Nerd Font required (avoids [] tofu)
        keymaps = {
          view = { { "n", "q", close, { desc = "Close diff" } }, { "n", "<esc>", close, {} } },
          file_panel = { { "n", "q", close, { desc = "Close diff" } }, { "n", "<esc>", close, {} } },
          file_history_panel = { { "n", "q", close, {} }, { "n", "<esc>", close, {} } },
        },
      })
    end,
  },

  -- Syntax + structural editing. nvim-treesitter rewrote its API: the old
  -- `nvim-treesitter.configs` module is gone. New API: setup(), install(), and
  -- highlighting via vim.treesitter.start() per buffer.
  {
    "nvim-treesitter/nvim-treesitter",
    branch = "main",
    build = ":TSUpdate",
    config = function()
      require("nvim-treesitter").setup()
      -- Install parsers asynchronously (no-op if already present).
      pcall(function()
        require("nvim-treesitter").install({
          "lua", "rust", "python", "bash", "markdown", "json", "toml", "c", "perl",
        })
      end)
      -- Enable treesitter highlighting + indentation when a parser exists.
      vim.api.nvim_create_autocmd("FileType", {
        callback = function(ev)
          pcall(vim.treesitter.start, ev.buf)
          pcall(function()
            vim.bo[ev.buf].indentexpr = "v:lua.require'nvim-treesitter'.indentexpr()"
          end)
        end,
      })
    end,
  },

  -- Peek window for LSP locations (Story #11): a VS Code-style popup showing
  -- the definition/references with a live preview, and Enter to jump there.
  -- Wired to gd/gr/gi/gt in the LspAttach block below.
  {
    "dnlhc/glance.nvim",
    cmd = "Glance",
    opts = {
      border = { enable = true },
      hooks = {
        -- If there's exactly one definition, jump straight there instead of
        -- opening a one-item popup.
        before_open = function(results, open, jump, method)
          if #results == 1 and method == "definitions" then
            jump(results[1])
          else
            open(results)
          end
        end,
      },
    },
  },

  -- Grouped results list (Story #36): find-callers opens here as a collapsible
  -- tree — each file shown once as a header with its references (line + text)
  -- nested underneath, <CR> to jump. Loaded on demand when find_callers calls
  -- require("trouble") (lazy hooks the require) or via :Trouble.
  {
    "folke/trouble.nvim",
    cmd = "Trouble",
    opts = {
      focus = true, -- move into the list so you can navigate it immediately
      keys = { ["<esc>"] = "close" }, -- Esc dismisses the window (as well as q)
    },
  },

  -- Find-all (and replace) across the repo (#53/#52): a live, grouped-by-file
  -- search buffer — type the query and matches appear grouped under each file
  -- as you type; fill the Replace field to rewrite across files. Bound to the
  -- repo-search key (Ctrl-G), which lazy-loads it on first press.
  {
    "MagicDuck/grug-far.nvim",
    cmd = "GrugFar",
    opts = {
      -- Open in its own tab: full editor pane (the default vsplit is half
      -- width), and closing returns to the file with no leftover [No Name]
      -- buffer (which `enew` left behind). Enter on a match (in normal mode)
      -- opens the real file at that location. Esc/submit maps are set on the
      -- buffer in the FileType autocmd below (#53).
      windowCreationCommand = "tab split",
      openTargetWindow = { useScratchBuffer = false },
    },
    keys = {
      {
        CFKeys.lhs("search_repo"),
        function()
          -- One reused instance ("codeforge-find"): reopening restores the last
          -- search text and its hits instead of a blank buffer. Esc hides (keeps
          -- the buffer), so the next Ctrl-G re-shows the same state (#53).
          local gf = require("grug-far")
          local name = "codeforge-find"
          if gf.has_instance(name) then
            gf.get_instance(name):open()
          else
            gf.open({ instanceName = name })
          end
        end,
        mode = { "n", "i" },
        desc = "Find all / replace (grug-far)",
      },
    },
  },

  -- LSP: go-to-definition, references (find callers), rename, etc.
  {
    "neovim/nvim-lspconfig",
    -- v2+ requires Neovim 0.11 and warns on 0.10; pin the last 0.10-friendly
    -- release. Drop this pin once on Neovim 0.11+.
    version = "v1.8.0",
    dependencies = {
      "williamboman/mason.nvim",
      -- Pin to 1.x to match nvim-lspconfig v1 (2.x rewrote the API).
      { "williamboman/mason-lspconfig.nvim", version = "v1.32.0" },
    },
    config = function()
      require("mason").setup()

      -- Servers to install (Story #11/#31). Mason installs these on first launch.
      --   rust -> rust_analyzer  python -> pyright   C/C++ -> clangd
      --   bash -> bashls         perl   -> perlnavigator   lua -> lua_ls
      --   html/css/json -> vscode-langservers   yaml -> yamlls
      --   toml -> taplo          markdown -> marksman
      -- Heavy servers that need a language runtime (Java jdtls, C# omnisharp,
      -- Ruby, Go gopls) are intentionally left out — install on demand via
      -- :Mason so a first launch without those runtimes doesn't fail noisily.
      local want = {
        "rust_analyzer", "pyright", "clangd", "bashls", "perlnavigator",
        "lua_ls", "html", "cssls", "jsonls", "yamlls", "taplo", "marksman",
      }

      -- Only ONE nvim auto-installs. CodeForge runs an nvim per pane/window, and
      -- all of them firing ensure_installed at once made Mason race — lockfile
      -- collisions and half-finished npm installs (#35). An atomic mkdir lock
      -- elects a single installer; the others skip (they'll see the servers once
      -- the installer finishes). A stale lock (crashed installer) is taken over
      -- after 10 minutes, and the holder releases it on exit.
      local lock = vim.fn.stdpath("state") .. "/codeforge-mason-install.lock"
      local st = vim.loop.fs_stat(lock)
      if st and (os.time() - st.mtime.sec) > 600 then
        pcall(vim.fn.delete, lock, "d")
      end
      -- mkdir is the atomic election: it *errors* (E739) if the dir already
      -- exists, so pcall succeeds only for the instance that created it.
      local installer = pcall(vim.fn.mkdir, lock)
      -- Force-install rather than ensure_installed so a package left
      -- half-installed by an interrupted run (bin symlink created, receipt not
      -- written) is cleaned and reinstalled instead of failing forever with
      -- '<bin> is already linked' on every launch (#48). We drive mason-registry
      -- directly for the force flag; if that API isn't present we fall back to
      -- plain ensure_installed.
      local self_heal = false
      if installer then
        vim.api.nvim_create_autocmd("VimLeavePre", {
          callback = function()
            pcall(vim.fn.delete, lock, "d")
          end,
        })
        local ok_map, servermap = pcall(require, "mason-lspconfig.mappings.server")
        local ok_reg, reg = pcall(require, "mason-registry")
        if ok_map and ok_reg then
          self_heal = true
          reg.refresh(vim.schedule_wrap(function()
            for _, server in ipairs(want) do
              local pkg = servermap.lspconfig_to_package[server]
              if pkg then
                local ok_p, p = pcall(reg.get_package, pkg)
                -- Only touch servers that aren't cleanly installed; force
                -- overwrites any leftover partial state.
                if ok_p and not p:is_installed() then
                  pcall(function()
                    p:install({ force = true })
                  end)
                end
              end
            end
          end))
        end
      end
      require("mason-lspconfig").setup({
        ensure_installed = (installer and not self_heal) and want or {},
      })

      -- Buffer-local LSP keymaps, set when a server attaches. Definition /
      -- references / implementation / type open in Glance's peek window (a
      -- preview popup you can jump from, per Story #11); <leader>-prefixed
      -- Telescope pickers remain as a list-style alternative.
      vim.api.nvim_create_autocmd("LspAttach", {
        callback = function(ev)
          local map = function(keys, fn, desc)
            vim.keymap.set("n", keys, fn, { buffer = ev.buf, desc = "LSP: " .. desc })
          end
          local tb = require("telescope.builtin")
          local has_glance = pcall(require, "glance")
          local peek = function(scope, fallback)
            return has_glance and ("<cmd>Glance " .. scope .. "<cr>") or fallback
          end
          -- Find callers, grouped by file (Story #36). Results go into the
          -- quickfix list and open in Trouble, which shows each file once as a
          -- header with its references (line + text) nested under it. We build
          -- the quickfix list ourselves so we control two things:
          --   * exclude the declaration (callers, not the definition itself);
          --   * fall back to a whole-word ripgrep when no attached server
          --     implements references (perlnavigator indexes definitions, not
          --     references), so Perl/shell get the same grouped view.
          local function open_callers_qf(title, items)
            if not items or #items == 0 then
              vim.notify("No callers found", vim.log.levels.INFO)
              return
            end
            vim.fn.setqflist({}, " ", { title = title, items = items })
            require("trouble").open("quickfix")
          end
          local function rg_callers(word)
            if word == "" then
              return
            end
            if vim.fn.executable("rg") == 0 then
              vim.notify("ripgrep (rg) not found; cannot search callers", vim.log.levels.WARN)
              return
            end
            local out = vim.fn.systemlist({ "rg", "--vimgrep", "--word-regexp", "--", word })
            local items = {}
            for _, line in ipairs(out) do
              local file, lnum, col, text = line:match("^(.-):(%d+):(%d+):(.*)$")
              if file then
                items[#items + 1] =
                  { filename = file, lnum = tonumber(lnum), col = tonumber(col), text = text }
              end
            end
            open_callers_qf("Callers: " .. word, items)
          end
          local function find_callers()
            local buf = vim.api.nvim_get_current_buf()
            local has_refs = false
            for _, c in pairs(vim.lsp.get_clients({ bufnr = buf })) do
              if c.server_capabilities and c.server_capabilities.referencesProvider then
                has_refs = true
                break
              end
            end
            if has_refs then
              vim.lsp.buf.references({ includeDeclaration = false }, {
                on_list = function(o)
                  open_callers_qf(o.title, o.items)
                end,
              })
            else
              rg_callers(vim.fn.expand("<cword>"))
            end
          end
          map("gd", peek("definitions", tb.lsp_definitions), "Peek / go to definition")
          map("gr", find_callers, "Find callers (references, no declaration)")
          map("gi", peek("implementations", tb.lsp_implementations), "Peek implementations")
          map("gt", peek("type_definitions", tb.lsp_type_definitions), "Peek type definition")
          -- Direct jump (no popup), for when you know where you're going.
          map("gD", vim.lsp.buf.definition, "Jump to definition")
          map("<leader>ds", tb.lsp_document_symbols, "Document symbols")
          map("<leader>ws", tb.lsp_dynamic_workspace_symbols, "Workspace symbols")
          map("K", vim.lsp.buf.hover, "Hover docs")
          map("<leader>rn", vim.lsp.buf.rename, "Rename symbol")
          map("<leader>ca", vim.lsp.buf.code_action, "Code action")
          map("[d", vim.diagnostic.goto_prev, "Prev diagnostic")
          map("]d", vim.diagnostic.goto_next, "Next diagnostic")

          -- Right-click context menu (#31): same navigation as the keys above,
          -- for the symbol under the click (mousemodel=popup_setpos moved the
          -- cursor there). Glance gives a peek window when installed, else the
          -- Telescope list — matching gd/gr. Works for every attached server
          -- (clangd/perlnavigator/rust_analyzer/pyright/bashls).
          local def_cmd = has_glance and "<Cmd>Glance definitions<CR>"
            or "<Cmd>lua require('telescope.builtin').lsp_definitions()<CR>"
          -- Menu strings can't reach the find_callers closure; expose it as a
          -- buffer-local command so "Find callers" gets the same declaration
          -- filter + ripgrep fallback as the gr map.
          vim.api.nvim_buf_create_user_command(ev.buf, "CFFindCallers", find_callers, {})
          vim.cmd("anoremenu PopUp.-CodeForgeSep- <Nop>")
          vim.cmd("anoremenu PopUp.Go\\ to\\ definition " .. def_cmd)
          vim.cmd("anoremenu PopUp.Find\\ callers <Cmd>CFFindCallers<CR>")
          vim.cmd("anoremenu PopUp.Rename\\ symbol <Cmd>lua vim.lsp.buf.rename()<CR>")
          vim.cmd("anoremenu PopUp.Code\\ action <Cmd>lua vim.lsp.buf.code_action()<CR>")
        end,
      })

      -- Enable any server that mason-lspconfig sees as installed.
      local ok, mlsp = pcall(require, "mason-lspconfig")
      if ok then
        local lspconfig = require("lspconfig")
        for _, server in ipairs(mlsp.get_installed_servers()) do
          local opts = {}
          if server == "lua_ls" then
            -- Editing this Neovim config is Lua: recognise the `vim` global so it
            -- isn't flagged undefined, and don't prompt about third-party libs.
            opts.settings = {
              Lua = {
                diagnostics = { globals = { "vim" } },
                workspace = { checkThirdParty = false },
              },
            }
          end
          lspconfig[server].setup(opts)
        end
      end
    end,
  },
}, {
  -- lazy.nvim UI/opts.
  ui = { border = "rounded" },
  change_detection = { notify = false },
  -- Check for plugin updates in the background, but don't nag on startup.
  -- Update on demand with <leader>u (mapped below).
  checker = { enabled = true, notify = false, frequency = 86400 },
})

-- Git diff (#18) keymaps live here, NOT inside the diffview plugin spec:
-- diffview lazy-loads on its :Diffview* commands, so a keymap defined in its
-- own config wouldn't exist until the command was run once by hand. Defined
-- globally, `<cmd>DiffviewOpen<cr>` both triggers the lazy load and opens it.
--   Space g d  changed-files panel + side-by-side diff (q / Esc closes)
--   Space g h  history of the current file      Space g c  close the diff
vim.keymap.set("n", "<leader>gd", "<cmd>DiffviewOpen<cr>", { desc = "Git diff (changed files)" })
vim.keymap.set("n", CFKeys.lhs("file_history"), "<cmd>DiffviewFileHistory %<cr>", { desc = "Git file history" })
vim.keymap.set("n", "<leader>gc", "<cmd>DiffviewClose<cr>", { desc = "Close git diff" })

-- Update plugins on demand (avoids auto-updating on every launch, which would
-- re-fetch once per nvim window and could break the editor silently).
vim.keymap.set("n", "<leader>u", "<cmd>Lazy update<cr>", { desc = "Update plugins" })

-- Project-wide search-and-replace scaffold (fill in the pattern).
vim.keymap.set("n", "<leader>rr", ":%s//g<Left><Left>", { desc = "Replace in file" })

-- Quick write/quit.
vim.keymap.set("n", "<leader>w", "<cmd>write<cr>", { desc = "Write" })
vim.keymap.set("n", "<leader>q", "<cmd>quit<cr>", { desc = "Quit window" })

-- Native CodeForge git diff (#18). Forge's Ctrl-a g list drives these over the
-- RPC socket. CodeForgeDiffOpen(file) builds a full-tab side-by-side diff —
-- the HEAD version read-only on the left, the real (editable) file on the
-- right, both in vim diff mode, plus a change map hugging the right edge —
-- and drops a flag file next to the RPC socket so forge knows the view is up.
-- Esc (or any way of killing the view) removes the flag, which is forge's cue
-- to restore its pane layout.
local function cf_diff_flag()
  return vim.v.servername .. ".diff"
end

-- State of the one open diff view; nil when closed.
local cf_diff = nil

-- Hunks vs HEAD as {start, count, kind} in working-file line numbers; kind is
-- "add" | "change" | "del" ("del" marks where lines were removed). Untracked
-- files are a single all-add hunk.
local function cf_diff_hunks()
  local st = cf_diff
  local total = math.max(vim.api.nvim_buf_line_count(st.right_buf), 1)
  if st.untracked then
    return { { start = 1, count = total, kind = "add" } }, total
  end
  local hunks = {}
  local raw = vim.fn.systemlist({ "git", "-C", st.root, "diff", "-U0", "HEAD", "--", st.rel })
  for _, l in ipairs(raw) do
    local _, oc, ns, nc = l:match("^@@ %-(%d+),?(%d*) %+(%d+),?(%d*) @@")
    if ns then
      ns = tonumber(ns)
      nc = nc ~= "" and tonumber(nc) or 1
      oc = oc ~= "" and tonumber(oc) or 1
      if nc == 0 then
        table.insert(hunks, { start = math.max(ns, 1), count = 1, kind = "del" })
      else
        table.insert(hunks, { start = ns, count = nc, kind = oc == 0 and "add" or "change" })
      end
    end
  end
  return hunks, total
end

-- Hunks change only when the file content does, so cache them on the diff state
-- and recompute only on open / save (#43). The old code ran `git diff` from
-- cf_diff_map_refresh, which fires on every WinScrolled — a git subprocess per
-- wheel notch, the source of the scroll glitchiness.
local function cf_diff_recompute_hunks()
  if not cf_diff then
    return
  end
  cf_diff.hunks, cf_diff.total = cf_diff_hunks()
end

-- The change map: a thin unfocusable float pinned to the right edge, one row
-- per slice of the file, colored where that slice contains changes. Rebuilt
-- whole on every refresh — it's tiny.
local cf_diff_map_ns = vim.api.nvim_create_namespace("codeforge_diff_map")
local CF_MAP_HL = { add = "DiffAdd", change = "DiffChange", del = "DiffDelete" }
local CF_MAP_RANK = { add = 1, change = 2, del = 3 }

local function cf_diff_map_refresh()
  local st = cf_diff
  if not (st and vim.api.nvim_win_is_valid(st.right_win) and vim.api.nvim_buf_is_valid(st.map_buf)) then
    return
  end
  -- winheight() excludes the winbar, which is exactly the strip we can cover.
  local h = vim.api.nvim_win_call(st.right_win, function()
    return vim.fn.winheight(0)
  end)
  local w = vim.api.nvim_win_get_width(st.right_win)
  if h < 2 or w < 4 then
    return
  end
  -- Use cached hunks (recomputed on open/save); only the thumb below is live
  -- per scroll. Lazily compute once if the cache is empty (first refresh).
  if not st.hunks then
    cf_diff_recompute_hunks()
  end
  local hunks, total = st.hunks, st.total
  if not (hunks and total) then
    return
  end
  local rows, marks = {}, {}
  for i = 1, h do
    rows[i] = " "
  end
  for _, hk in ipairs(hunks) do
    local r1 = math.floor((hk.start - 1) * h / total) + 1
    local r2 = math.floor((hk.start + hk.count - 2) * h / total) + 1
    for r = math.max(r1, 1), math.min(r2, h) do
      if not marks[r] or CF_MAP_RANK[hk.kind] > CF_MAP_RANK[marks[r]] then
        marks[r] = hk.kind
      end
      rows[r] = "▐"
    end
  end
  -- Scroll thumb: a dim left-half bar over the rows currently on screen.
  -- Where the viewport crosses a change block the cell becomes a full block.
  local first, last = unpack(vim.api.nvim_win_call(st.right_win, function()
    return { vim.fn.line("w0"), vim.fn.line("w$") }
  end))
  local t1 = math.max(math.floor((first - 1) * h / total) + 1, 1)
  local t2 = math.min(math.floor((last - 1) * h / total) + 1, h)
  for r = t1, t2 do
    rows[r] = rows[r] == "▐" and "█" or "▌"
  end
  vim.bo[st.map_buf].modifiable = true
  vim.api.nvim_buf_set_lines(st.map_buf, 0, -1, false, rows)
  vim.bo[st.map_buf].modifiable = false
  vim.api.nvim_buf_clear_namespace(st.map_buf, cf_diff_map_ns, 0, -1)
  for r, kind in pairs(marks) do
    vim.api.nvim_buf_set_extmark(st.map_buf, cf_diff_map_ns, r - 1, 0, {
      end_col = #"▐",
      hl_group = CF_MAP_HL[kind],
    })
  end
  local cfg = {
    relative = "win",
    win = st.right_win,
    anchor = "NW",
    row = 1, -- window-relative row 0 is the winbar; the map covers the text rows
    col = w - 1,
    width = 1,
    height = h,
    focusable = false,
    style = "minimal",
    zindex = 30,
  }
  if st.map_win and vim.api.nvim_win_is_valid(st.map_win) then
    vim.api.nvim_win_set_config(st.map_win, cfg)
  else
    st.map_win = vim.api.nvim_open_win(st.map_buf, false, cfg)
    vim.wo[st.map_win].winhighlight = "Normal:Comment"
  end
end

-- Zoom the focused side to (almost) full width, or back to 50/50.
local function cf_diff_focus_toggle()
  if vim.api.nvim_win_get_width(0) < vim.o.columns - 12 then
    vim.cmd("wincmd |")
  else
    vim.cmd("wincmd =")
  end
end

-- Step to the next/prev changed file's diff without leaving the view (#51).
-- Walks the same set the forge diff list shows (`git diff --name-only HEAD`),
-- wrapping around, and reopens the diff on that file. `]`/`[` are bound to this;
-- `]c`/`[c` stay the native within-file hunk motions (nvim disambiguates).
local function cf_diff_step(delta)
  local st = cf_diff
  if not st then
    return
  end
  -- Use the cached changed-files list (refreshed on open/save) so stepping
  -- doesn't re-shell it each time (#56).
  local files = st.files
  if not files or #files == 0 then
    files = vim.fn.systemlist({ "git", "-C", st.root, "diff", "--name-only", "HEAD" })
    st.files = files
  end
  if not files or #files == 0 then
    return
  end
  local idx = 1
  for i, f in ipairs(files) do
    if f == st.rel then
      idx = i
      break
    end
  end
  local nxt = ((idx - 1 + delta) % #files) + 1
  -- Swap the file in the open view instead of tearing it down and rebuilding
  -- (#56) — much snappier, and it keeps the flag so forge sees no close.
  _G.CodeForgeDiffSwap(st.root .. "/" .. files[nxt])
end

-- The diff view's buffer-local keys, applied to each side on open and to the
-- new right buffer after a swap (#56). Factored so open and swap stay in sync.
local function cf_diff_apply_buf_maps(b)
  vim.keymap.set("n", "<Esc>", _G.CodeForgeDiffClose, { buffer = b, nowait = true, silent = true })
  vim.keymap.set("n", "<Tab>", function()
    local st = cf_diff
    if st then
      local cur = vim.api.nvim_get_current_win()
      local other = cur == st.right_win and st.left_win or st.right_win
      if vim.api.nvim_win_is_valid(other) then
        vim.api.nvim_set_current_win(other)
      end
    end
  end, { buffer = b, nowait = true, silent = true })
  vim.keymap.set("n", "<leader>z", cf_diff_focus_toggle, { buffer = b, silent = true })
  vim.keymap.set("n", "]", function()
    cf_diff_step(1)
  end, { buffer = b, nowait = true, silent = true })
  vim.keymap.set("n", "[", function()
    cf_diff_step(-1)
  end, { buffer = b, nowait = true, silent = true })
  vim.keymap.set("n", "<CR>", function()
    return vim.fn.foldclosed(".") ~= -1 and "za" or "<CR>"
  end, { buffer = b, expr = true, silent = true })
end

-- Swap the file shown in an already-open diff view, reusing its tab, windows and
-- left scratch instead of the full teardown+rebuild CodeForgeDiffOpen does
-- (#56). Only `git show HEAD:<file>` runs (root + file list are cached); the
-- flag is left in place so forge never sees a close. Falls back to a full open
-- if no view is live.
function _G.CodeForgeDiffSwap(path)
  local st = cf_diff
  if not (st and vim.api.nvim_win_is_valid(st.left_win) and vim.api.nvim_win_is_valid(st.right_win)) then
    return _G.CodeForgeDiffOpen(path)
  end
  local rel = path:sub(#st.root + 2)
  local head = vim.fn.systemlist({ "git", "-C", st.root, "show", "HEAD:" .. rel })
  local untracked = false
  if vim.v.shell_error ~= 0 then
    head = {}
    untracked = true
  end

  local old_right = st.right_buf
  vim.api.nvim_set_current_win(st.right_win)
  vim.cmd("edit " .. vim.fn.fnameescape(path))
  local right_buf = vim.api.nvim_get_current_buf()
  local ft = vim.bo.filetype

  vim.bo[st.left_buf].modifiable = true
  vim.api.nvim_buf_set_lines(st.left_buf, 0, -1, false, head)
  pcall(vim.api.nvim_buf_set_name, st.left_buf, "HEAD: " .. rel)
  vim.bo[st.left_buf].modifiable = false
  vim.bo[st.left_buf].filetype = ft

  -- Re-diff both sides for the new buffer.
  vim.api.nvim_win_call(st.left_win, function()
    vim.cmd("diffthis")
  end)
  vim.api.nvim_win_call(st.right_win, function()
    vim.cmd("diffthis")
  end)

  st.rel, st.untracked, st.right_buf = rel, untracked, right_buf
  cf_diff_apply_buf_maps(right_buf)
  vim.api.nvim_create_autocmd("BufWritePost", {
    group = st.aug,
    buffer = right_buf,
    callback = function()
      vim.schedule(function()
        st.files = vim.fn.systemlist({ "git", "-C", st.root, "diff", "--name-only", "HEAD" })
        cf_diff_recompute_hunks()
        cf_diff_map_refresh()
      end)
    end,
  })
  -- Drop the previous file buffer so they don't accumulate as you step.
  if old_right and old_right ~= right_buf and vim.api.nvim_buf_is_valid(old_right) then
    pcall(vim.api.nvim_buf_delete, old_right, {})
  end
  cf_diff_recompute_hunks()
  cf_diff_map_refresh()
  return "ok"
end

function _G.CodeForgeDiffOpen(path)
  if vim.g.codeforge_diff_tab and vim.api.nvim_tabpage_is_valid(vim.g.codeforge_diff_tab) then
    _G.CodeForgeDiffClose()
  end
  local dir = vim.fn.fnamemodify(path, ":h")
  local root = (vim.fn.systemlist({ "git", "-C", dir, "rev-parse", "--show-toplevel" }) or {})[1]
  if vim.v.shell_error ~= 0 or not root or root == "" then
    return "err: not a git repository"
  end
  local rel = path:sub(#root + 2)
  local head = vim.fn.systemlist({ "git", "-C", root, "show", "HEAD:" .. rel })
  local untracked = false
  if vim.v.shell_error ~= 0 then
    head = {} -- new/untracked file: the whole right side shows as added
    untracked = true
  end

  vim.cmd("tab split")
  local tab = vim.api.nvim_get_current_tabpage()
  vim.cmd("edit " .. vim.fn.fnameescape(path))
  local right_win = vim.api.nvim_get_current_win()
  local right_buf = vim.api.nvim_get_current_buf()
  local ft = vim.bo.filetype

  -- Left half: the HEAD version as a read-only scratch that wipes on close.
  vim.cmd("leftabove vnew")
  local left_win = vim.api.nvim_get_current_win()
  local left_buf = vim.api.nvim_get_current_buf()
  vim.api.nvim_buf_set_lines(left_buf, 0, -1, false, head)
  pcall(vim.api.nvim_buf_set_name, left_buf, "HEAD: " .. rel)
  local bo = vim.bo[left_buf]
  bo.buftype = "nofile"
  bo.bufhidden = "wipe"
  bo.swapfile = false
  bo.modifiable = false
  bo.filetype = ft
  vim.b[left_buf].codeforge_diff_left = true
  vim.cmd("diffthis")
  vim.cmd("wincmd l")
  vim.cmd("diffthis")

  -- Label the sides and put the keys where they're discoverable. Softer
  -- filler glyph for lines that only exist on the other side.
  vim.wo[left_win].winbar = "  HEAD · read-only"
  vim.wo[right_win].winbar = "  %t · ]/[ next/prev file · Tab side · Esc list"
  vim.wo[left_win].fillchars = "diff: "
  vim.wo[right_win].fillchars = "diff: "
  -- The global scrolloff (6) pins the cursor that many rows below the top, so
  -- with scrollbind the current-code side can never reach line 1. Drop it to 0
  -- on both diff windows so either side scrolls fully to its own top (#44).
  vim.wo[left_win].scrolloff = 0
  vim.wo[right_win].scrolloff = 0

  local aug = vim.api.nvim_create_augroup("codeforge_diff", { clear = true })
  local map_buf = vim.api.nvim_create_buf(false, true)
  vim.bo[map_buf].bufhidden = "wipe"

  cf_diff = {
    tab = tab,
    root = root,
    rel = rel,
    untracked = untracked,
    left_buf = left_buf,
    right_buf = right_buf,
    left_win = left_win,
    right_win = right_win,
    map_buf = map_buf,
    map_win = nil,
    aug = aug,
    -- Changed-file list for ]/[ stepping, cached so it isn't re-shelled each
    -- step; refreshed here and on save (#56).
    files = vim.fn.systemlist({ "git", "-C", root, "diff", "--name-only", "HEAD" }),
  }

  -- Esc closes; Tab hops sides; Space-z zooms; ] / [ step files; Enter folds
  -- (#51/#56, factored so a swap re-applies the same set). `nowait` on ] / [
  -- fires them instantly instead of waiting timeoutlen for ]b/]d/[b/[d.
  cf_diff_apply_buf_maps(left_buf)
  cf_diff_apply_buf_maps(right_buf)

  -- Forge opens the diff while the pane is still small, then zooms it — nvim
  -- dumps all the new columns on the rightmost window, so re-equalize on every
  -- terminal resize while the diff tab is up.
  vim.api.nvim_create_autocmd("VimResized", {
    group = aug,
    callback = function()
      if cf_diff and vim.api.nvim_get_current_tabpage() == cf_diff.tab then
        vim.cmd("wincmd =")
        -- WinResized doesn't reliably chain off wincmd here; re-anchor directly.
        vim.schedule(cf_diff_map_refresh)
      end
    end,
  })
  vim.api.nvim_create_autocmd("WinResized", {
    group = aug,
    callback = function()
      local st = cf_diff
      if not st then
        return
      end
      for _, w in ipairs(vim.v.event.windows or {}) do
        if w == st.right_win or w == st.left_win then
          vim.schedule(cf_diff_map_refresh)
          return
        end
      end
    end,
  })
  -- vim only syncs scrollbind windows when the *current* window scrolls, so a
  -- wheel over the unfocused half moves it alone. Re-sync from whichever side
  -- actually moved, then redraw the thumb.
  vim.api.nvim_create_autocmd("WinScrolled", {
    group = aug,
    pattern = { tostring(left_win), tostring(right_win) },
    callback = function(ev)
      local st = cf_diff
      local win = tonumber(ev.match)
      if st and win and vim.api.nvim_win_is_valid(win) then
        vim.api.nvim_win_call(win, function()
          vim.cmd("syncbind")
        end)
      end
      vim.schedule(cf_diff_map_refresh)
    end,
  })
  -- Autosave keeps disk current, so the map tracks edits as they land. This is
  -- the only place hunks are recomputed (git diff) besides open — scroll/resize
  -- reuse the cache (#43).
  vim.api.nvim_create_autocmd("BufWritePost", {
    group = aug,
    buffer = right_buf,
    callback = function()
      vim.schedule(function()
        if cf_diff then
          cf_diff.files = vim.fn.systemlist({ "git", "-C", root, "diff", "--name-only", "HEAD" })
        end
        cf_diff_recompute_hunks()
        cf_diff_map_refresh()
      end)
    end,
  })

  -- Teardown is anchored to the left scratch: however the view dies (Esc,
  -- :tabclose, :q), wiping that buffer removes the flag and all the trimmings.
  vim.api.nvim_create_autocmd("BufWipeout", {
    buffer = left_buf,
    once = true,
    callback = function()
      local st = cf_diff
      cf_diff = nil
      vim.g.codeforge_diff_tab = nil
      if st then
        pcall(vim.api.nvim_del_augroup_by_id, st.aug)
        if st.map_win and vim.api.nvim_win_is_valid(st.map_win) then
          pcall(vim.api.nvim_win_close, st.map_win, true)
        end
        for _, k in ipairs({ "<Esc>", "<Tab>", "<leader>z", "<CR>", "]", "[" }) do
          pcall(vim.keymap.del, "n", k, { buffer = st.right_buf })
        end
        -- The right window survives the last-tab fallback close: un-dress it.
        if vim.api.nvim_win_is_valid(st.right_win) then
          vim.wo[st.right_win].winbar = ""
          vim.wo[st.right_win].fillchars = ""
        end
      end
      pcall(vim.cmd, "diffoff!")
      vim.fn.delete(cf_diff_flag())
    end,
  })

  vim.g.codeforge_diff_tab = tab
  cf_diff_map_refresh()
  vim.fn.writefile({}, cf_diff_flag())
  return "ok"
end

function _G.CodeForgeDiffClose()
  local tab = vim.g.codeforge_diff_tab
  if tab and vim.api.nvim_tabpage_is_valid(tab) then
    local closed = pcall(vim.cmd, "tabclose " .. vim.api.nvim_tabpage_get_number(tab))
    if not closed then
      -- Only tab left (the original got closed meanwhile): drop the scratch
      -- window instead; its wipe runs the teardown autocmd above.
      for _, win in ipairs(vim.api.nvim_tabpage_list_wins(tab)) do
        -- Closing one window can invalidate others still in the pre-fetched
        -- list (and swaps churn buffers), so re-check before touching each.
        if vim.api.nvim_win_is_valid(win) then
          local b = vim.api.nvim_win_get_buf(win)
          if vim.b[b].codeforge_diff_left then
            pcall(vim.api.nvim_win_close, win, true)
          end
        end
      end
      pcall(vim.cmd, "diffoff!")
    end
  end
  return "ok"
end

-- An nvim exiting mid-diff must not leave a stale flag behind (forge would
-- keep the editor zoomed waiting for it).
vim.api.nvim_create_autocmd("VimLeavePre", {
  callback = function()
    vim.fn.delete(cf_diff_flag())
  end,
})

-- Fullscreen the editor while a grug-far search is open (#53): write a flag
-- forge watches (see grug_flag in main.rs), removed when the grug-far buffer
-- goes away or nvim exits. servername == the --listen socket, so the flag path
-- matches forge's `<sock>.grug`.
local function cf_grug_flag()
  return vim.v.servername .. ".grug"
end
vim.api.nvim_create_autocmd("FileType", {
  pattern = "grug-far",
  callback = function(ev)
    local buf = ev.buf
    -- Enter while typing = "submit": leave insert (results already update live),
    -- so you can step to hits without reaching for Esc first (#53).
    vim.keymap.set("i", "<CR>", "<Esc>", { buffer = buf, desc = "grug-far: submit search" })
    -- Esc in normal mode hides the window but keeps the instance, so reopening
    -- restores the search + hits (kill would lose them) (#53).
    vim.keymap.set("n", "<Esc>", function()
      require("grug-far").hide_instance(buf)
    end, { buffer = buf, nowait = true, desc = "grug-far: hide" })

    if vim.v.servername == "" then
      return
    end
    -- Flag tracks window *visibility*, not buffer creation: the reused instance's
    -- buffer outlives a hide, so keying off show/hide makes forge fullscreen the
    -- editor on every open, not just the first (#53). BufWinLeave fires only when
    -- the window closes (Esc/hide) — moving focus to an opened match leaves it up.
    vim.api.nvim_create_autocmd({ "BufWinEnter", "WinEnter" }, {
      buffer = buf,
      callback = function()
        vim.fn.writefile({}, cf_grug_flag())
      end,
    })
    vim.api.nvim_create_autocmd("BufWinLeave", {
      buffer = buf,
      callback = function()
        pcall(vim.fn.delete, cf_grug_flag())
      end,
    })
    vim.fn.writefile({}, cf_grug_flag())
  end,
})
vim.api.nvim_create_autocmd("VimLeavePre", {
  callback = function()
    pcall(vim.fn.delete, cf_grug_flag())
  end,
})

-- Autosave (#19): write real file buffers on change / leaving insert, unless
-- CodeForge disabled it via `g:codeforge_autosave = 0` (config `autosave = false`).
if vim.g.codeforge_autosave ~= 0 then
  vim.api.nvim_create_autocmd({ "TextChanged", "InsertLeave", "FocusLost", "BufLeave" }, {
    callback = function(ev)
      local bo = vim.bo[ev.buf]
      local name = vim.api.nvim_buf_get_name(ev.buf)
      -- Don't create a file on disk for a brand-new, still-empty buffer — e.g. a
      -- fresh Notes note you haven't written yet. No blank notes get committed
      -- (#70). (Emptying an existing file still saves.)
      local empty = vim.api.nvim_buf_line_count(ev.buf) == 1
        and (vim.api.nvim_buf_get_lines(ev.buf, 0, 1, false)[1] or "") == ""
      if empty and name ~= "" and vim.fn.filereadable(name) == 0 then
        return
      end
      if
        bo.buftype == "" -- a normal file buffer (not oil, alpha, terminal, …)
        and bo.modifiable
        and bo.modified
        and not bo.readonly
        and name ~= ""
      then
        -- `silent!` so a no-write situation (e.g. a new no-name split) is quiet.
        vim.cmd("silent! noautocmd write")
      end
    end,
    desc = "Autosave",
  })
end
