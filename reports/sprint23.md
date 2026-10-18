# Sprint 23 — Fase 1: gramática de seletores e cascata

Escopo: Fase 1 do plano mestre (`local://naro-master-roadmap-plan.md`), executada
por inteiro. Fases 2–7 não foram tocadas.

## O que mudou

| # | Mudança | Arquivo |
|---|---|---|
| 1 | Modelo de seletor: `Combinator`, `AttrSelector`/`AttrOp`, `Pseudo`, `Compound` com `attrs`+`pseudos`, `Selector` com `combinators`, `SkipReason`, `ParseReport`, `Stylesheet::report` | `naro-css` |
| 2 | Parser de seletor com listas por vírgula em profundidade zero, combinadores ` `, `>`, `+`, `~`, atributos (`=`, `~=`, `^=`, `$=`, `*=`, `|=`, sufixo `i`), `:root`, `:first-child`, `:last-child`, `:nth-child(an+b)`, `:not()` | `naro-css` |
| 3 | Matching da direita para a esquerda com os quatro combinadores; `:nth-child` aritmético com `a` negativo; pseudo-classes de interação nunca casam | `naro-css` |
| 4 | `!important` deixa de fazer parte do valor e entra na ordenação: UA normal < autor normal < inline normal < autor `!important` < inline `!important` | `naro-css` |
| 5 | `querySelector`/`querySelectorAll` movidos para `naro-css`; a duplicata de `naro-js` (gramática própria, sem combinadores nem listas) foi removida | `naro-css`, `naro-js` |
| 6 | `Document::element_index`, `previous_element_sibling`, `next_element_sibling` | `naro-html` |
| 7 | Linha `CSS:` no `--dump`; `css_report` no `Engine` | `naro-engine` |
| 8 | Check `runcolor` no runner de compatibilidade; página `23-selectors.html` | `naro-app`, `tests/compat` |

## Verificação

- `cargo test --workspace`: **87 testes**, 0 falhas (65 anteriores + 22 novos).
- `naro --compat tests/compat`: **22 pass, 0 falhas, 1 unsup (total 23)** — era 21/0/1.
- `--dump tests/compat/23-selectors.html`:
  `CSS: rules=10 invalid=0 pseudo-element=0 unsupported-not=0 at-rule=0 interactive=1`,
  `link estatico` em `#1a56db` (UA, não o `#ff0000` do `a:hover`), `campo texto` em
  `#1558d6` (`input[type=text]`), `importante vence` em `#0f766e` (o `!important`
  venceu a regra posterior).
- `reports/s23-selectors.png`: título verde, segundo card vermelho, campo azul,
  "estado ligado" roxo, itens da lista em duas cores, "importante vence" em teal.

## Regressão nas páginas públicas (baseline × depois)

Dumps: `reports/baseline-*.txt` (binário anterior) e `reports/after-*.txt`.

| Página | STATUS | CONTENT_HEIGHT | JS ERROR | Textos | Classificação |
|---|---|---|---|---|---|
| `naro://start` | Idle → Idle | 606,40 → 606,40 | nenhum | idênticos | sem mudança |
| `example.com` | Idle → Idle | 172,80 → 172,80 | `s.js: not a function` nos dois | idênticos | `JS ERROR` **pré-existente** |
| `rust-lang.org` | Idle → Idle | 3233,16 → 3233,16 | nenhum | um run do `<select>` de idioma deixou de ser duplicado | esperado |
| `google.com` | Idle → Idle | 484,00 → 484,00 | nenhum | idênticos | esperado (ver abaixo) |
| MDN `/Web/HTML` | Idle → Idle | 11322,00 → 11322,00 | 6 erros nos dois, conjunto idêntico | mesmos textos, 7 títulos 33% maiores | esperado |

Causas confirmadas por CSS da própria página:

- **Google:** os dois botões perdiam `#dadce0` de `.lsb:active{background:#dadce0}`.
  Antes `:active` era removido e a regra virava `.lsb`; agora o estado não vaza para
  a pintura estática. `CSS: rules=250 invalid=0 pseudo-element=3 at-rule=2 interactive=20`.
  A diferença de cor do botão "Fazer login" entre `google_s22.png` e `google_s23.png`
  **não** é regressão: no dump antigo essa página também não tinha bloco azul
  (`#0b57d0` não aparece em `reports/baseline-google.txt`); `google_s22.png` foi
  gerado a partir de outra variante servida pelo Google (botão em y≈37–74, contra
  o link em y≈17 do carregamento atual).
- **rust-lang:** `.select:after{position:absolute;width:0;height:0;…}` deixava de ser
  pseudo-elemento e era aplicado ao próprio `.select`, que virava uma caixa de
  largura zero em x=992; por isso os `<option>` do seletor de idioma eram pintados
  como cinco runs sobrepostos. Agora a regra é descartada (contada como
  `pseudo-element`), o wrapper mantém a posição normal e o texto aparece uma vez.
- **MDN:** títulos de navegação passam de 14,2/18 px para 19/24 px (×1,333) porque
  uma regra com pseudo-classe de interação deixou de rebaixar o `font-size` herdado.
  Nenhum texto perdido, nenhum erro novo.

## Relatório de descarte (por que uma folha não aplica)

`--dump` passou a publicar, depois de `STATUS:`:

```
CSS: rules=N invalid=I pseudo-element=P unsupported-not=U at-rule=A interactive=V unsupported-pseudo=S
```

Exemplo em produção: MDN carrega `rules=437 pseudo-element=57 unsupported-not=11
at-rule=116 interactive=70 unsupported-pseudo=182`; nenhuma regra desaparece sem
motivo contabilizado.

## Próximo passo

Fase 2 (unidades e `calc()`), conforme o roadmap.
