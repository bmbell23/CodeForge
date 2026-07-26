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

-- Sensible baseline.
local opt = vim.opt
opt.number = true
opt.relativenumber = false -- show real (absolute) line numbers on every line
opt.mouse = "a"
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

-- Bootstrap lazy.nvim.
local lazypath = vim.fn.stdpath("data") .. "/lazy/lazy.nvim"
if not vim.loop.fs_stat(lazypath) then
  vim.fn.system({
    "git", "clone", "--filter=blob:none",
    "https://github.com/folke/lazy.nvim.git", "--branch=stable", lazypath,
  })
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
      require("telescope").setup({
        defaults = { preview = { treesitter = false } },
      })
      local t = require("telescope.builtin")
      -- VS Code-style (work in normal & insert):
      --   Ctrl-P        open file by name
      --   Ctrl-F        search within the current file
      --   Ctrl-Shift-F  search the whole repo (needs a terminal that sends the
      --                 Shift chord; <leader>fg always works as a fallback)
      vim.keymap.set({ "n", "i" }, "<C-p>", t.find_files, { desc = "Open file" })
      vim.keymap.set({ "n", "i" }, "<C-f>", t.current_buffer_fuzzy_find, { desc = "Search in file" })
      vim.keymap.set({ "n", "i" }, "<C-S-f>", t.live_grep, { desc = "Search repo" })
      vim.keymap.set("n", "<leader>ff", t.find_files, { desc = "Find files" })
      vim.keymap.set("n", "<leader>fg", t.live_grep, { desc = "Find all (grep repo)" })
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
      vim.keymap.set("n", "<leader>e", "<cmd>Oil<cr>", { desc = "File explorer" })
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
        },
      })
      -- Cycle / reorder / close buffers. ]b [b move; <leader>bp pins a picker.
      vim.keymap.set("n", "]b", "<cmd>BufferLineCycleNext<cr>", { desc = "Next buffer/tab" })
      vim.keymap.set("n", "[b", "<cmd>BufferLineCyclePrev<cr>", { desc = "Prev buffer/tab" })
      vim.keymap.set("n", "<S-l>", "<cmd>BufferLineCycleNext<cr>", { desc = "Next buffer/tab" })
      vim.keymap.set("n", "<S-h>", "<cmd>BufferLineCyclePrev<cr>", { desc = "Prev buffer/tab" })
      vim.keymap.set("n", "<leader>bp", "<cmd>BufferLinePick<cr>", { desc = "Pick a buffer/tab" })
      vim.keymap.set("n", "<leader>bd", "<cmd>bdelete<cr>", { desc = "Close buffer/tab" })
      -- Jump straight to tab N.
      for i = 1, 9 do
        vim.keymap.set(
          "n",
          "<leader>" .. i,
          "<cmd>BufferLineGoToBuffer " .. i .. "<cr>",
          { desc = "Go to buffer/tab " .. i }
        )
      end
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

      -- Project + git context line.
      local function git_bits()
        local br = vim.fn.systemlist({ "git", "-C", cwd, "branch", "--show-current" })
        if vim.v.shell_error ~= 0 or not br[1] or br[1] == "" then
          return nil, 0
        end
        local st = vim.fn.systemlist({ "git", "-C", cwd, "status", "--porcelain" })
        return br[1], (vim.v.shell_error == 0) and #st or 0
      end
      local branch, changed = git_bits()
      local ctx = "  " .. vim.fn.fnamemodify(cwd, ":t")
      if branch then
        ctx = ctx .. "   ⎇ " .. branch
        if changed > 0 then
          ctx = ctx .. "   " .. changed .. " changed"
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
      local buttons = {}
      for i = 1, math.min(#recent, 6) do
        local f = recent[i]
        local disp = vim.fn.fnamemodify(f, ":~:.")
        buttons[i] = db.button(tostring(i), "  " .. disp, "<cmd>edit " .. vim.fn.fnameescape(f) .. "<cr>")
      end

      -- Grouped, aligned cheatsheet. Space = leader; Ctrl-a = CodeForge prefix.
      local keys = {
        "Files    Ctrl-P  open file       Space e   file tree     Space fg  search text",
        "Editor   gd  go to definition    gr  references          ]b / [b   prev / next file",
        "Git      Space gd  diff view (changed files, side-by-side, editable)",
        "Panes    Ctrl-a e/t/c  editor / terminal / claude        Ctrl-a hjkl  move focus",
        "Session  Ctrl-a n  new window     Ctrl-a v  scroll / copy   Ctrl-a ?  all keys",
      }
      local footer = { "just start typing to fuzzy-open a file   ·   Space e for the file tree" }

      local function text(lines, hl)
        return { type = "text", val = lines, opts = { position = "center", hl = hl } }
      end
      local layout = {
        { type = "padding", val = 2 },
        text(header, "Keyword"),
        { type = "padding", val = 1 },
        text({ ctx }, "Identifier"),
        { type = "padding", val = 1 },
      }
      if #buttons > 0 then
        layout[#layout + 1] = text({ "recent files — press its number to open" }, "Comment")
        layout[#layout + 1] = { type = "group", val = buttons, opts = { spacing = 0 } }
        layout[#layout + 1] = { type = "padding", val = 1 }
      end
      layout[#layout + 1] = text(keys, "Comment")
      layout[#layout + 1] = { type = "padding", val = 1 }
      layout[#layout + 1] = text(footer, "Function")
      alpha.setup({ layout = layout, opts = { margin = 5 } })

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
            -- start(false) forces a draw; start(true) (the on-vimenter path)
            -- proved to be a no-op with this alpha version.
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
              require("alpha").start(false)
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
      require("mason-lspconfig").setup({
        -- The languages we want working out of the box (Story #11). Mason
        -- installs these servers on first launch; add more here or via :Mason.
        --   rust  -> rust_analyzer   python -> pyright     C -> clangd
        --   bash  -> bashls          perl   -> perlnavigator
        ensure_installed = {
          "rust_analyzer",
          "pyright",
          "clangd",
          "bashls",
          "perlnavigator",
        },
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
          map("gd", peek("definitions", tb.lsp_definitions), "Peek / go to definition")
          map("gr", peek("references", tb.lsp_references), "Peek references (callers)")
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
        end,
      })

      -- Enable any server that mason-lspconfig sees as installed.
      local ok, mlsp = pcall(require, "mason-lspconfig")
      if ok then
        local lspconfig = require("lspconfig")
        for _, server in ipairs(mlsp.get_installed_servers()) do
          lspconfig[server].setup({})
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
vim.keymap.set("n", "<leader>gh", "<cmd>DiffviewFileHistory %<cr>", { desc = "Git file history" })
vim.keymap.set("n", "<leader>gc", "<cmd>DiffviewClose<cr>", { desc = "Close git diff" })

-- Update plugins on demand (avoids auto-updating on every launch, which would
-- re-fetch once per nvim window and could break the editor silently).
vim.keymap.set("n", "<leader>u", "<cmd>Lazy update<cr>", { desc = "Update plugins" })

-- Project-wide search-and-replace scaffold (fill in the pattern).
vim.keymap.set("n", "<leader>rr", ":%s//g<Left><Left>", { desc = "Replace in file" })

-- Quick write/quit.
vim.keymap.set("n", "<leader>w", "<cmd>write<cr>", { desc = "Write" })
vim.keymap.set("n", "<leader>q", "<cmd>quit<cr>", { desc = "Quit window" })

-- Autosave (#19): write real file buffers on change / leaving insert, unless
-- CodeForge disabled it via `g:codeforge_autosave = 0` (config `autosave = false`).
if vim.g.codeforge_autosave ~= 0 then
  vim.api.nvim_create_autocmd({ "TextChanged", "InsertLeave", "FocusLost", "BufLeave" }, {
    callback = function(ev)
      local bo = vim.bo[ev.buf]
      if
        bo.buftype == "" -- a normal file buffer (not oil, alpha, terminal, …)
        and bo.modifiable
        and bo.modified
        and not bo.readonly
        and vim.api.nvim_buf_get_name(ev.buf) ~= ""
      then
        -- `silent!` so a no-write situation (e.g. a new no-name split) is quiet.
        vim.cmd("silent! noautocmd write")
      end
    end,
    desc = "Autosave",
  })
end
