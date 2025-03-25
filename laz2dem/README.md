Usage:

```
Usage: laz2dem [OPTIONS] --bbox <BBOX> --zoom-level <ZOOM_LEVEL> --shadings <SHADINGS> <--laz-tile-db <LAZ_TILE_DB>|--laz-index-db <LAZ_INDEX_DB>> <OUTPUT>

Arguments:
  <OUTPUT>  Output mbtiles file

Options:
      --laz-tile-db <LAZ_TILE_DB>
          Source as LAZ tile DB
      --laz-index-db <LAZ_INDEX_DB>
          Source as LAZ index DB referring *.laz files
      --bbox <BBOX>
          EPSG:3857 bounding box to render
      --source-projection <SOURCE_PROJECTION>
          Projection of points if reading from *.laz; default is EPSG:3857
      --zoom-level <ZOOM_LEVEL>
          Max zoom level of tiles to generate
      --unit-zoom-level <UNIT_ZOOM_LEVEL>
          If LAZ tile DB is used then use value of `--zoom-level` argument of `laztile` If LAZ index is used then use zoom level to determine size of tile to process at once [default: 16]
      --shadings <SHADINGS>
          Shadings; `+` separated componets of shading. Shading component is <method>,method_param1[,method_param2...].
          ‎
          Methods:
          - `oblique` - params: azimuth in degrees, alitutde in degrees
          - `igor` - params: azimuth in degrees
          - `slope` - params: alitutde in degrees
      --contrast <CONTRAST>
          Increase (> 1.0) or decrease (< 1.0) contrast of the shading. Use value higher than 0.0 [default: 1]
      --brightness <BRIGHTNESS>
          Increase (> 0.0) or decrease (< 0.0) brightness of the shading. Use value between -1.0 and 1.0 [default: 0]
      --z-factor <Z_FACTOR>
          Z-factor [default: 1]
      --tile-size <TILE_SIZE>
          Tile size [default: 256]
      --buffer <BUFFER>
          Buffer size in pixels to prevent artifacts at tieledges [default: 40]
      --format <FORMAT>
          Tile image format. For alpha (transparency) support use `png` [default: jpeg] [possible values: jpeg, png]
      --jpeg-quality <JPEG_QUALITY>
          Quality from 0 to 100 when writing to JPEG [default: 80]
      --background-color <BACKGROUND_COLOR>
          Background color when writing to JPEG as it does not support alpha [default: FFFFFF]
  -h, --help
          Print help
```

Example:

```sh
cargo run --release -- --supertile-zoom-offset 4 --laz-tile-db /home/martin/14TB/sk-new-dmr/laztiles.sqlite --bbox  2347219,6223449,2356002,6228760 test.mbtile --zoom-level 20 --shadings igor,203060E0,-120+igor,FFEE00D0,60+igor,000000FF,-45
```
