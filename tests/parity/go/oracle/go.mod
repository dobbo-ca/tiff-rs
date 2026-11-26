module tiff-parity-oracle

go 1.27.1

require (
	github.com/dobbo-ca/deflate-go v0.4.0
	github.com/dobbo-ca/jpeg-go v0.8.0
	github.com/dobbo-ca/tiff-go v0.0.0
	golang.org/x/image v0.47.0
)

replace github.com/dobbo-ca/tiff-go => ../upstream

replace github.com/dobbo-ca/deflate-go => ../deps/deflate-go

replace github.com/dobbo-ca/jpeg-go => ../deps/jpeg-go

replace golang.org/x/image => ../deps/x-image
