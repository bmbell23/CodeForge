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
          "lua", "rust", "python", "bash", "markdown", "json", "toml", "c",
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
        -- Installed on demand; add servers here or via :Mason.
        ensure_installed = {},
      })

      -- Buffer-local LSP keymaps, set when a server attaches.
      vim.api.nvim_create_autocmd("LspAttach", {
        callback = function(ev)
          local map = function(keys, fn, desc)
            vim.keymap.set("n", keys, fn, { buffer = ev.buf, desc = "LSP: " .. desc })
          end
          local tb = require("telescope.builtin")
          map("gd", tb.lsp_definitions, "Go to definition")
          map("gr", tb.lsp_references, "Find references (callers)")
          map("gi", tb.lsp_implementations, "Go to implementation")
          map("gt", tb.lsp_type_definitions, "Go to type definition")
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
