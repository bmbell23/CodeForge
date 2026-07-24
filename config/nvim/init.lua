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
opt.relativenumber = true
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
      local t = require("telescope.builtin")
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
    opts = { view_options = { show_hidden = true } },
    config = function(_, opts)
      require("oil").setup(opts)
      vim.keymap.set("n", "<leader>e", "<cmd>Oil<cr>", { desc = "File explorer" })
      vim.keymap.set("n", "-", "<cmd>Oil<cr>", { desc = "Open parent dir" })
    end,
  },

  -- Syntax + structural editing.
  {
    "nvim-treesitter/nvim-treesitter",
    -- Pin the stable branch: the rewritten `main` branch dropped the
    -- `nvim-treesitter.configs` setup API this config uses.
    branch = "master",
    build = ":TSUpdate",
    config = function()
      require("nvim-treesitter.configs").setup({
        ensure_installed = { "lua", "rust", "python", "bash", "markdown", "json", "toml" },
        auto_install = true,
        highlight = { enable = true },
        indent = { enable = true },
      })
    end,
  },

  -- LSP: go-to-definition, references (find callers), rename, etc.
  {
    "neovim/nvim-lspconfig",
    dependencies = {
      "williamboman/mason.nvim",
      "williamboman/mason-lspconfig.nvim",
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
})

-- Project-wide search-and-replace scaffold (fill in the pattern).
vim.keymap.set("n", "<leader>rr", ":%s//g<Left><Left>", { desc = "Replace in file" })

-- Quick write/quit.
vim.keymap.set("n", "<leader>w", "<cmd>write<cr>", { desc = "Write" })
vim.keymap.set("n", "<leader>q", "<cmd>quit<cr>", { desc = "Quit window" })
