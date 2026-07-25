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
      -- rewritten nvim-treesitter removed — turn off treesitter in the preview
      -- so live_grep/find_files don't crash.
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
    dependencies = { "nvim-tree/nvim-web-devicons" },
    config = function()
      vim.opt.termguicolors = true
      require("bufferline").setup({
        options = {
          diagnostics = "nvim_lsp",
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

-- Update plugins on demand (avoids auto-updating on every launch, which would
-- re-fetch once per nvim window and could break the editor silently).
vim.keymap.set("n", "<leader>u", "<cmd>Lazy update<cr>", { desc = "Update plugins" })

-- Project-wide search-and-replace scaffold (fill in the pattern).
vim.keymap.set("n", "<leader>rr", ":%s//g<Left><Left>", { desc = "Replace in file" })

-- Quick write/quit.
vim.keymap.set("n", "<leader>w", "<cmd>write<cr>", { desc = "Write" })
vim.keymap.set("n", "<leader>q", "<cmd>quit<cr>", { desc = "Quit window" })
